#[derive(Clone, BotCommands)]
#[command(rename_rule = "lowercase")]
enum Command {
    #[command(description = "show what Jossie can do")]
    Start,
    #[command(description = "show available commands")]
    Help,
    #[command(description = "start a fresh conversation")]
    New,
    #[command(description = "stop the current run")]
    Cancel,
    #[command(description = "show what we're currently working on")]
    Status,
    #[command(description = "resume the latest safely paused goal")]
    Resume,
}

#[derive(Default)]
struct TelegramRuntime {
    active_chats: Mutex<HashSet<i64>>,
    albums: Mutex<HashMap<(i64, String), PendingAlbum>>,
    secretary_batches: Mutex<HashMap<(String, i64, bool), PendingSecretaryBatch>>,
}

struct PendingSecretaryBatch {
    generation: u64,
    messages: Vec<teloxide::types::Message>,
}

struct PendingAlbum {
    generation: u64,
    messages: Vec<teloxide::types::Message>,
}

pub struct TelegramBot {
    token: String,
    state: Arc<AppState>,
    allowed_user_id: Option<i64>,
    secretary_rx: Option<tokio::sync::mpsc::Receiver<jossie_server::TelegramSecretaryCommand>>,
}

impl TelegramBot {
    pub fn new(
        token: &str,
        state: Arc<AppState>,
        allowed_user_id: Option<i64>,
        secretary_rx: Option<
            tokio::sync::mpsc::Receiver<jossie_server::TelegramSecretaryCommand>,
        >,
    ) -> Self {
        Self {
            token: token.trim().to_string(),
            state,
            allowed_user_id,
            secretary_rx,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let bot = Bot::new(&self.token);
        let runtime = Arc::new(TelegramRuntime::default());
        let state = self.state;
        let allowed_user_id = self.allowed_user_id;
        let secretary_rx = self.secretary_rx;

        if state.llm.transcription_is_configured() {
            match tokio::process::Command::new(&state.telegram.ffmpeg_path)
                .arg("-version")
                .output()
                .await
            {
                Ok(output) if output.status.success() => {}
                Ok(_) | Err(_) => tracing::warn!(
                    ffmpeg = %state.telegram.ffmpeg_path,
                    "FFmpeg is unavailable; Telegram voice notes will be rejected"
                ),
            }
        }

        let message_state = state.clone();
        let message_runtime = runtime.clone();
        let message_handler = Update::filter_message().endpoint(
            move |bot: Bot, msg: teloxide::types::Message| {
                let state = message_state.clone();
                let runtime = message_runtime.clone();
                async move {
                    if !authorized_message(&msg, allowed_user_id) {
                        return respond_ok();
                    }
                    if !msg.chat.is_private() {
                        if command_from_message(&msg).is_some() {
                            let _ = bot
                                .send_message(msg.chat.id, "Please message me in a private chat.")
                                .await;
                        }
                        return respond_ok();
                    }

                    if try_handle_secretary_revision(&bot, &state, &msg).await {
                        return respond_ok();
                    }

                    if let Some(command) = command_from_message(&msg) {
                        handle_command(bot, state, runtime, msg, command).await;
                        return respond_ok();
                    }

                    if let Some(group_id) = msg.media_group_id().map(ToString::to_string) {
                        queue_album(bot, state, runtime, msg, group_id).await;
                        return respond_ok();
                    }

                    if !try_activate_chat(&runtime, msg.chat.id.0).await {
                        let _ = send_reply(
                            &bot,
                            msg.chat.id,
                            Some(msg.id),
                            "I'm still working on your previous message. Use /cancel if you want me to stop.",
                            None,
                        )
                        .await;
                        return respond_ok();
                    }

                    let chat_id = msg.chat.id.0;
                    tokio::spawn(async move {
                        process_turn(bot, state, vec![msg]).await;
                        release_chat(&runtime, chat_id).await;
                    });
                    respond_ok()
                }
            },
        );

        let callback_state = state.clone();
        let callback_runtime = runtime.clone();
        let callback_handler =
            Update::filter_callback_query().endpoint(move |bot: Bot, query: CallbackQuery| {
                let state = callback_state.clone();
                let runtime = callback_runtime.clone();
                async move {
                    if !authorized_user_id(query.from.id.0 as i64, allowed_user_id) {
                        return respond_ok();
                    }
                    tokio::spawn(async move {
                        handle_callback(bot, state, runtime, query).await;
                    });
                    respond_ok()
                }
            });

        let connection_state = state.clone();
        let connection_handler = Update::filter_business_connection().endpoint(
            move |bot: Bot, connection: teloxide::types::BusinessConnection| {
                let state = connection_state.clone();
                async move {
                    handle_secretary_connection(bot, state, connection, true).await;
                    respond_ok()
                }
            },
        );

        let business_state = state.clone();
        let business_runtime = runtime.clone();
        let business_handler = Update::filter_business_message().endpoint(
            move |bot: Bot, message: teloxide::types::Message| {
                let state = business_state.clone();
                let runtime = business_runtime.clone();
                async move {
                    queue_secretary_message(bot, state, runtime, message).await;
                    respond_ok()
                }
            },
        );

        let edited_state = state.clone();
        let edited_handler = Update::filter_edited_business_message().endpoint(
            move |bot: Bot, message: teloxide::types::Message| {
                let state = edited_state.clone();
                async move {
                    handle_edited_secretary_message(bot, state, message).await;
                    respond_ok()
                }
            },
        );

        let deleted_state = state.clone();
        let deleted_handler = Update::filter_deleted_business_messages().endpoint(
            move |bot: Bot, deleted: teloxide::types::BusinessMessagesDeleted| {
                let state = deleted_state.clone();
                async move {
                    handle_deleted_secretary_messages(bot, state, deleted).await;
                    respond_ok()
                }
            },
        );

        let handler = dptree::entry()
            .branch(message_handler)
            .branch(callback_handler)
            .branch(connection_handler)
            .branch(business_handler)
            .branch(edited_handler)
            .branch(deleted_handler);

        tracing::info!(instance_id = %Uuid::new_v4(), "Starting Telegram bot");
        bot.delete_webhook().await?;
        if state.telegram.secretary_enabled {
            let me = bot.get_me().await?;
            anyhow::ensure!(
                me.can_connect_to_business,
                "Telegram secretary is enabled locally but Secretary Mode is disabled in @BotFather"
            );
            for saved in state.db.list_telegram_business_connections().await? {
                match bot
                    .get_business_connection(BusinessConnectionId(saved.id.clone()))
                    .await
                {
                    Ok(connection) => {
                        handle_secretary_connection(bot.clone(), state.clone(), connection, false)
                            .await;
                    }
                    Err(error) => tracing::warn!(
                        connection_id = %saved.id,
                        "Could not reconcile Telegram secretary connection at startup: {error}"
                    ),
                }
            }
        }
        bot.set_my_commands(Command::bot_commands()).await?;

        let goal_notification_task = tokio::spawn(run_goal_notification_loop(
            bot.clone(),
            state.clone(),
            runtime.clone(),
            state.event_tx.subscribe(),
        ));
        let secretary_send_task = secretary_rx.map(|receiver| {
            tokio::spawn(run_secretary_send_loop(bot.clone(), receiver))
        });

        let mut listener = update_listeners::polling_default(bot.clone()).await;
        let stop_token = listener.stop_token();
        let polling_conflict = Arc::new(AtomicBool::new(false));
        let conflict_seen = polling_conflict.clone();
        let listener_error_handler = Arc::new(move |error: RequestError| {
            let stop_token = stop_token.clone();
            let conflict_seen = conflict_seen.clone();
            async move {
                if is_polling_conflict(&error) {
                    conflict_seen.store(true, Ordering::Release);
                    tracing::error!(
                        "Telegram polling stopped because another process is using this bot token; stop the other bot instance or configure a unique token"
                    );
                    stop_token.stop();
                } else {
                    tracing::warn!(error = ?error, "Telegram update listener error; polling will retry");
                }
            }
        });

        Dispatcher::builder(bot, handler)
            .build()
            .dispatch_with_listener(listener, listener_error_handler)
            .await;
        goal_notification_task.abort();
        if let Some(task) = secretary_send_task {
            task.abort();
        }

        if polling_conflict.load(Ordering::Acquire) {
            anyhow::bail!(
                "another Telegram getUpdates consumer is using this bot token; only one polling instance may run"
            );
        }
        Ok(())
    }
}
