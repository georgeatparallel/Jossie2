/// Poll the durable continuation queue independently of integration polling.
pub async fn start_work_continuations(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = dispatch_work_continuations(&state).await {
            tracing::warn!("Could not dispatch work continuations: {error}");
        }
    }
}

async fn dispatch_work_continuations(
    state: &Arc<AppState>,
) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    let mut workers = Vec::new();
    for checkpoint in state.db.list_queued_work_checkpoints().await? {
        let conv_id = Uuid::parse_str(&checkpoint.conversation_id)?;
        // Reserve synchronously before spawning: concurrent ticks cannot launch
        // duplicates, and foreground chat can preempt this background lease.
        let Some(lease) = state.active_conversations.claim_background(conv_id) else {
            continue;
        };
        let saved: serde_json::Value = serde_json::from_str(&checkpoint.state_json)?;
        let mut options: AgentRunOptions = saved
            .get("execution_options")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        options.work_source_type = Some("goal_continuation".into());
        options.work_source_id = Some(checkpoint.run_id.clone());
        options.goal_id = Some(checkpoint.goal_id.clone());
        options.task_id = checkpoint.task_id;
        options.resume_checkpoint_run_id = Some(checkpoint.run_id.clone());
        state.clear_cancel(conv_id).await;
        state
            .run_cancellations
            .write()
            .await
            .insert(conv_id, lease.cancellation.clone());
        let state = state.clone();
        workers.push(tokio::spawn(async move {
            let result = run_agent_loop_with_lease(&state, conv_id, options, lease).await;
            match result {
                Ok(response) if !response.trim().is_empty() => {
                    // The assistant reply is already durable in the conversation.
                    // Queue only its Telegram delivery; do not save it a second time.
                    if !state.telegram.token.trim().is_empty() {
                        if let Err(error) = state.db.queue_continuation_result_delivery(conv_id, &response).await {
                            tracing::warn!("Could not queue continuation result delivery: {error}");
                        }
                    }
                }
                Ok(_) => {} // Another slice has been queued; work remains active.
                Err(error) => {
                    tracing::warn!(goal_id = %checkpoint.goal_id, "Continuation will retry when available: {error}");
                    // Claims are released by the run wrapper. Avoid a tight retry
                    // loop for failures while retaining the durable queued work.
                    let _ = state.db.defer_work_checkpoint_retry(&checkpoint.run_id).await;
                }
            }
        }));
    }
    Ok(workers)
}
