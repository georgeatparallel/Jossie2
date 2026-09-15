/// Evaluation is separate from delivery: no notification is recorded as sent here.
pub enum EventEvaluation {
    Skip,
    Notify(EventNotification),
    Deferred(String),
    Retry(String),
    Failed(String),
    InspectionFailed(Vec<EmailInspectionFailure>),
}

#[derive(Debug, Clone)]
pub struct EmailInspectionFailure {
    /// None means the batch entry could not be decoded into an event.
    pub event_id: Option<String>,
    pub detail: String,
    pub permanent: bool,
}

pub struct EventNotification {
    pub message: String,
    pub fingerprint: String,
    pub evaluated_at: chrono::DateTime<chrono::Utc>,
    // Keep ownership until the caller either delivers or defers this decision.
    pub(crate) lease: crate::conversation_runs::ConversationLease,
}

impl EventNotification {
    pub fn is_current(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        !self.lease.cancellation.is_cancelled()
            && now.signed_duration_since(self.evaluated_at) < chrono::Duration::seconds(60)
    }
}

#[derive(Debug)]
pub enum EventEvaluationError {
    Deferred(String),
    Retry(String),
    Permanent(String),
    Inspection(Vec<EmailInspectionFailure>),
}

impl std::fmt::Display for EventEvaluationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deferred(error) | Self::Retry(error) | Self::Permanent(error) => {
                formatter.write_str(error)
            }
            Self::Inspection(errors) => formatter.write_str(
                &errors
                    .iter()
                    .map(|error| error.detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
        }
    }
}
impl std::error::Error for EventEvaluationError {}

impl EventEvaluation {
    pub fn into_result(self) -> anyhow::Result<Option<EventNotification>> {
        match self {
            Self::Skip => Ok(None),
            Self::Notify(notification) => Ok(Some(notification)),
            Self::Deferred(error) => Err(EventEvaluationError::Deferred(error).into()),
            Self::Retry(error) => Err(EventEvaluationError::Retry(error).into()),
            Self::Failed(error) => Err(EventEvaluationError::Permanent(error).into()),
            Self::InspectionFailed(errors) => Err(EventEvaluationError::Inspection(errors).into()),
        }
    }
}

pub fn event_error_is_deferred(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<EventEvaluationError>(),
        Some(EventEvaluationError::Deferred(_))
    ) || is_conversation_busy(error)
}

pub fn event_error_is_permanent(error: &anyhow::Error) -> bool {
    if matches!(
        error.downcast_ref::<EventEvaluationError>(),
        Some(EventEvaluationError::Permanent(_))
    ) {
        return true;
    }
    if error.downcast_ref::<serde_json::Error>().is_some() {
        return true;
    }
    // Match explicit provider failures, never arbitrary numbers in an email body.
    let message = format!("{error:#}").to_lowercase();
    [
        "401 unauthorized",
        "403 forbidden",
        "404 not found",
        "invalid_grant",
        "email not found",
        "unknown or unconfigured mail provider",
        "event is missing",
        "unsupported mail account id",
        "message reference provider mismatch",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

/// Background requests are retried by the durable queue with fresh context.
async fn complete_event_iteration(
    state: &AppState,
    messages: &[Message],
    tools: &[jossie_core::ToolDefinition],
    prompt_cache_key: &str,
    format: &jossie_llm::StructuredOutputFormat,
) -> anyhow::Result<jossie_llm::LlmOutput> {
    let mut messages = messages.to_vec();
    messages.push(Message::transient(Role::System, format!(
        "Current decision time: {}. Use explicit event dates, times, and time zones. Do not give relative countdowns such as 'in N minutes'. Reassess whether this event still needs attention now.",
        chrono::Utc::now().to_rfc3339(),
    )));
    let options = jossie_llm::LlmRequestOptions {
        max_rate_limit_retries: Some(0),
        structured_output: Some(format.clone()),
        ..initial_llm_request_options(state, prompt_cache_key)
    };
    tokio::time::timeout(
        Duration::from_secs(state.agent.llm_request_timeout_seconds.min(60)),
        state.llm.complete_with_options(&messages, tools, &options),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Background LLM request timed out"))?
}
