async fn run_secretary_send_loop(
    bot: Bot,
    mut receiver: tokio::sync::mpsc::Receiver<jossie_server::TelegramSecretaryCommand>,
) {
    while let Some(command) = receiver.recv().await {
        let mut request = bot
            .send_message(ChatId(command.chat_id), command.text)
            .business_connection_id(BusinessConnectionId(command.business_connection_id));
        if let Some(message_id) = command.reply_to_message_id {
            request = request.reply_parameters(
                ReplyParameters::new(MessageId(message_id)).allow_sending_without_reply(),
            );
        }
        let result = request.await.map(|message| jossie_server::TelegramSecretarySendResult {
            message_id: message.id.0,
            sent_at: message.date,
        });
        let _ = command.response.send(result.map_err(anyhow::Error::from));
    }
}

async fn handle_secretary_connection(
    bot: Bot,
    state: Arc<AppState>,
    connection: teloxide::types::BusinessConnection,
    notify_owner: bool,
) {
    if !state.telegram.secretary_enabled {
        return;
    }
    let owner_user_id = connection.user.id.0 as i64;
    if state.telegram.allowed_user_id != Some(owner_user_id) {
        tracing::warn!(owner_user_id, "Ignoring Telegram secretary connection from a non-owner");
        return;
    }
    let connection_id = connection.id.0.clone();
    if !connection.is_enabled {
        match state
            .db
            .purge_telegram_business_connection(&connection_id)
            .await
        {
            Ok(paths) => {
                for path in paths {
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
            Err(error) => {
                tracing::error!(connection_id, "Failed to purge disconnected secretary data: {error}");
            }
        }
        return;
    }
    let rights = serde_json::to_value(connection.rights.as_ref()).unwrap_or_default();
    let connected_at = connection.date.to_rfc3339();
    if let Err(error) = state
        .db
        .upsert_telegram_business_connection(
            &connection_id,
            owner_user_id,
            connection.user_chat_id.0 as i64,
            &rights,
            true,
            &connected_at,
        )
        .await
    {
        tracing::error!(connection_id, "Failed to save Telegram secretary connection: {error}");
        return;
    }
    let can_reply = rights
        .get("can_reply")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let text = if can_reply {
        "Secretary mode is connected. I’ll draft replies for review and will never send one without your approval."
    } else {
        "Secretary mode is connected, but reply permission is disabled. Enable reply access in Telegram to approve and send drafts."
    };
    if notify_owner {
        let _ = bot
            .send_message(ChatId(connection.user_chat_id.0 as i64), text)
            .await;
    }
}

async fn queue_secretary_message(
    bot: Bot,
    state: Arc<AppState>,
    runtime: Arc<TelegramRuntime>,
    message: teloxide::types::Message,
) {
    if !state.telegram.secretary_enabled || !message.chat.is_private() {
        return;
    }
    let Some(connection_id) = secretary_connection_id(&message).map(|id| id.0.clone())
    else {
        return;
    };
    let Some(connection) = state
        .db
        .get_telegram_business_connection(&connection_id)
        .await
        .ok()
        .flatten()
        .filter(|connection| connection.is_enabled)
    else {
        tracing::warn!(connection_id, "Ignoring message for unknown secretary connection");
        return;
    };
    let inbound = message
        .from
        .as_ref()
        .is_none_or(|sender| sender.id.0 as i64 != connection.owner_user_id);
    let key = (connection_id, message.chat.id.0, inbound);
    let generation = {
        let mut batches = runtime.secretary_batches.lock().await;
        let batch = batches.entry(key.clone()).or_insert(PendingSecretaryBatch {
            generation: 0,
            messages: Vec::new(),
        });
        batch.generation += 1;
        batch.messages.push(message);
        batch.generation
    };
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        let messages = {
            let mut batches = runtime.secretary_batches.lock().await;
            let Some(batch) = batches.get(&key) else { return };
            if batch.generation != generation {
                return;
            }
            batches.remove(&key).map(|batch| batch.messages)
        };
        if let Some(messages) = messages
            && let Err(error) = process_secretary_messages(&bot, &state, messages, inbound).await
        {
            tracing::error!("Telegram secretary message processing failed: {error:#}");
        }
    });
}

async fn process_secretary_messages(
    bot: &Bot,
    state: &Arc<AppState>,
    messages: Vec<teloxide::types::Message>,
    inbound: bool,
) -> anyhow::Result<()> {
    let first = messages.first().context("Empty secretary message batch")?;
    let connection_id = secretary_connection_id(first)
        .context("Business message lacks a connection id")?
        .0
        .clone();
    let connection = state
        .db
        .get_telegram_business_connection(&connection_id)
        .await?
        .filter(|connection| connection.is_enabled)
        .context("Secretary connection is unavailable")?;
    if connection.owner_user_id != state.telegram.allowed_user_id.unwrap_or_default() {
        anyhow::bail!("Secretary connection does not belong to the configured owner");
    }
    let chat_id = first.chat.id.0;
    let peer_name = [first.chat.first_name(), first.chat.last_name()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let peer_name = if peer_name.trim().is_empty() {
        first.chat.username().unwrap_or("Telegram contact").to_string()
    } else {
        peer_name
    };
    let chat = state
        .db
        .get_or_create_telegram_business_chat(
            &connection_id,
            chat_id,
            &peer_name,
            first.chat.username(),
        )
        .await?;
    let mut unseen = Vec::new();
    for message in messages {
        if !state
            .db
            .telegram_business_message_exists(&connection_id, chat_id, message.id.0)
            .await?
        {
            unseen.push(message);
        }
    }
    if unseen.is_empty() {
        return Ok(());
    }

    let local_media = download_media_group(bot, state, &unseen).await?;
    let content = match build_user_content(state, &unseen, &local_media).await {
        Ok(content) => content,
        Err(error) => {
            cleanup_local_media(state, &local_media).await;
            return Err(error);
        }
    };
    if content.trim().is_empty() && local_media.is_empty() {
        return Ok(());
    }
    let attachments = local_media
        .iter()
        .map(|media| Attachment {
            id: media.id,
            name: media.name.clone(),
            mime_type: Some(media.mime_type.clone()),
            size: media.size as i64,
            data: None,
        })
        .collect::<Vec<_>>();
    let role = if inbound { Role::User } else { Role::Assistant };
    let mut message = JossieMessage::new(chat.conversation_id, role, content);
    if !attachments.is_empty() {
        message = message.with_attachments(attachments);
    }
    if !inbound {
        message = message.with_name("telegram_secretary_owner".to_string());
    }
    persist_sensitive_media_message(state, &message, &local_media).await?;
    for telegram_message in &unseen {
        state
            .db
            .record_telegram_business_message(
                &connection_id,
                chat_id,
                telegram_message.id.0,
                message.id,
                if inbound { "inbound" } else { "outbound" },
                &telegram_message.date.to_rfc3339(),
            )
            .await?;
    }
    state
        .db
        .supersede_secretary_drafts(chat.conversation_id, "The Telegram chat changed")
        .await?;
    if !inbound {
        return Ok(());
    }
    let latest_chat = state
        .db
        .get_telegram_business_chat(&connection_id, chat_id)
        .await?
        .context("Managed chat disappeared")?;
    let action = jossie_server::create_secretary_draft(
        state,
        &latest_chat,
        unseen.last().map(|message| message.id.0),
    )
    .await?;
    notify_owner_of_secretary_draft(bot, &connection, &latest_chat, &action).await?;
    Ok(())
}

async fn notify_owner_of_secretary_draft(
    bot: &Bot,
    connection: &jossie_db::TelegramBusinessConnection,
    chat: &jossie_db::TelegramBusinessChat,
    action: &jossie_db::PendingAction,
) -> anyhow::Result<MessageId> {
    let draft = action.editable_text.as_deref().unwrap_or_default();
    let text = format!(
        "Draft reply to {}\n\n{}\n\n{}\n\nSecretary draft {}",
        chat.peer_name, draft, action.summary, action.id
    );
    let can_reply = connection
        .rights
        .get("can_reply")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let keyboard = if can_reply {
        pending_keyboard(std::slice::from_ref(action))
    } else {
        InlineKeyboardMarkup::new([vec![InlineKeyboardButton::callback(
            "Reject",
            format!("pa:n:{}:{}", action.id, action.revision),
        )]])
    };
    let message = bot
        .send_message(ChatId(connection.owner_chat_id), text)
        .reply_markup(keyboard)
        .await?;
    Ok(message.id)
}

async fn handle_edited_secretary_message(
    bot: Bot,
    state: Arc<AppState>,
    message: teloxide::types::Message,
) {
    let Some(connection_id) = secretary_connection_id(&message).map(|id| id.0.clone()) else {
        return;
    };
    let content = message.caption().or_else(|| message.text()).unwrap_or_default();
    match state
        .db
        .update_telegram_business_message(&connection_id, message.chat.id.0, message.id.0, content)
        .await
    {
        Ok(Some(_)) => {
            if let Ok(Some(chat)) = state
                .db
                .get_telegram_business_chat(&connection_id, message.chat.id.0)
                .await
            {
                let _ = state
                    .db
                    .supersede_secretary_drafts(chat.conversation_id, "The source message was edited")
                    .await;
                if message
                    .from
                    .as_ref()
                    .is_some_and(|sender| Some(sender.id.0 as i64) != state.telegram.allowed_user_id)
                    && let Ok(action) = jossie_server::create_secretary_draft(
                        &state,
                        &chat,
                        Some(message.id.0),
                    )
                    .await
                    && let Ok(Some(connection)) = state
                        .db
                        .get_telegram_business_connection(&connection_id)
                        .await
                {
                    let _ = notify_owner_of_secretary_draft(&bot, &connection, &chat, &action).await;
                }
            }
        }
        Ok(None) => {}
        Err(error) => tracing::error!("Failed to update edited secretary message: {error}"),
    }
}

fn secretary_connection_id(
    message: &teloxide::types::Message,
) -> Option<&BusinessConnectionId> {
    match &message.kind {
        teloxide::types::MessageKind::Common(common) => {
            common.business_connection_id.as_ref()
        }
        _ => None,
    }
}

async fn handle_deleted_secretary_messages(
    _bot: Bot,
    state: Arc<AppState>,
    deleted: teloxide::types::BusinessMessagesDeleted,
) {
    let connection_id = deleted.business_connection_id.0;
    let ids = deleted.message_ids.iter().map(|id| id.0).collect::<Vec<_>>();
    let paths = match state
        .db
        .mark_telegram_business_messages_deleted(&connection_id, deleted.chat.id.0, &ids)
        .await
    {
        Ok(paths) => paths,
        Err(error) => {
            tracing::error!("Failed to apply deleted secretary messages: {error}");
            return;
        }
    };
    for path in paths {
        if let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path, "Failed to remove deleted secretary attachment: {error}");
        }
    }
    if let Ok(Some(chat)) = state
        .db
        .get_telegram_business_chat(&connection_id, deleted.chat.id.0)
        .await
    {
        let _ = state
            .db
            .supersede_secretary_drafts(chat.conversation_id, "The source message was deleted")
            .await;
    }
}

async fn try_handle_secretary_revision(
    bot: &Bot,
    state: &Arc<AppState>,
    message: &teloxide::types::Message,
) -> bool {
    let Some(instruction) = message.text().map(str::trim).filter(|text| !text.is_empty()) else {
        return false;
    };
    let Some(replied) = message.reply_to_message() else {
        return false;
    };
    let Some(review_text) = replied.text() else {
        return false;
    };
    let Some(action_id) = review_text
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("Secretary draft "))
    else {
        return false;
    };
    let Ok(Some(action)) = state.db.get_pending_action(action_id.trim()).await else {
        return false;
    };
    if action.kind != "telegram_secretary_reply" {
        return false;
    }
    match jossie_server::revise_secretary_draft(state, &action, instruction).await {
        Ok(updated) => {
            if let Ok(Some(chat)) = state
                .db
                .get_telegram_business_chat_for_conversation(updated.conversation_id)
                .await
                && let Ok(Some(connection)) = state
                    .db
                    .get_telegram_business_connection(&chat.connection_id)
                    .await
            {
                let _ = notify_owner_of_secretary_draft(bot, &connection, &chat, &updated).await;
            }
        }
        Err(error) => {
            let _ = bot
                .send_message(message.chat.id, format!("I couldn't revise that draft: {error}"))
                .await;
        }
    }
    true
}
