async fn finish_event_failure(
    db: &jossie_db::Database,
    events: &[IntegrationEvent],
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    for event in events {
        if let Some(jossie_server::agent::EventEvaluationError::Inspection(failures)) =
            error.downcast_ref()
        {
            match failures
                .iter()
                .find(|failure| failure.event_id.as_deref().is_none_or(|id| id == event.id))
            {
                Some(failure) if failure.permanent => {
                    db.mark_integration_event_failed(&event.id, &failure.detail)
                        .await?
                }
                Some(failure) => {
                    db.retry_integration_event(&event.id, &failure.detail)
                        .await?
                }
                None => db.mark_integration_event_new(&event.id).await?,
            }
        } else if jossie_server::agent::event_error_is_deferred(error) {
            db.mark_integration_event_new(&event.id).await?;
        } else if jossie_server::agent::event_error_is_permanent(error) {
            db.mark_integration_event_failed(&event.id, &format!("{error:#}"))
                .await?;
        } else {
            db.retry_integration_event(&event.id, &format!("{error:#}"))
                .await?;
        }
    }
    if !jossie_server::agent::event_error_is_deferred(error) {
        db.record_activity_event(
            None,
            None,
            "background",
            "Event evaluation needs attention",
            Some(&format!("{error:#}")),
            "warning",
        )
        .await?;
    }
    Ok(())
}

async fn deliver_event_notification(
    state: &Arc<AppState>,
    target: &BackgroundTarget,
    source: &str,
    notification: jossie_server::agent::EventNotification,
    events: &[IntegrationEvent],
) -> anyhow::Result<()> {
    use jossie_server::agent::EventEvaluationError;
    if !notification.is_current(Utc::now()) {
        return Err(EventEvaluationError::Deferred(
            "Notification decision expired or foreground work arrived".to_string(),
        )
        .into());
    }
    let ids: Vec<_> = events.iter().map(|event| event.id.clone()).collect();
    if let Some(chat_id) = target
        .telegram_chat_id
        .filter(|_| !state.telegram.token.trim().is_empty())
    {
        state.db.begin_integration_event_delivery(&ids).await?;
        if !notification.is_current(Utc::now()) {
            state.db.clear_integration_event_delivery(&ids).await?;
            return Err(EventEvaluationError::Deferred(
                "Notification decision superseded before delivery".to_string(),
            )
            .into());
        }
        if let Err(error) = jossie_telegram::send_notification(
            &state.telegram.token,
            chat_id,
            &notification.message,
        )
        .await
        {
            use jossie_telegram::NotificationDeliveryError;
            match error {
                NotificationDeliveryError::Retryable(message) => {
                    state.db.clear_integration_event_delivery(&ids).await?;
                    return Err(EventEvaluationError::Retry(message).into());
                }
                NotificationDeliveryError::Permanent(message) => {
                    state.db.clear_integration_event_delivery(&ids).await?;
                    return Err(EventEvaluationError::Permanent(message).into());
                }
                NotificationDeliveryError::Uncertain(message) => {
                    return Err(EventEvaluationError::Permanent(format!(
                        "Delivery outcome uncertain; no automatic resend: {message}"
                    ))
                    .into());
                }
            }
        }
    }
    // Persist before marking the fingerprint delivered. A failure after a send
    // retains delivery_started_at so neither retry nor restart repeats the send.
    let assistant_msg = Message {
        id: Uuid::new_v4(),
        conversation_id: target.conversation_id,
        role: Role::Assistant,
        content: notification.message.clone(),
        tool_calls: None,
        tool_call_id: None,
        name: Some("integration_event_notification".to_string()),
        attachments: None,
        response_items: None,
        created_at: Utc::now(),
    };
    persist_message(state, &assistant_msg).await?;
    jossie_server::agent::record_event_notification(
        state,
        target.conversation_id,
        &notification.fingerprint,
    )
    .await?;
    state
        .publish_event(ServerEvent::BackgroundNotification {
            conversation_id: target.conversation_id,
            source: source.to_string(),
            message: assistant_msg.content,
        })
        .await;
    mark_events_processed(state, events).await?;
    Ok(())
}
