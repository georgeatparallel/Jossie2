// Non-Flex callers retain a bounded retry policy. Flex keeps waiting for capacity.
const MAX_RATE_LIMIT_RETRIES: usize = 13;
const BASE_RETRY_DELAY_SECS: u64 = 2;
const MAX_RETRY_DELAY_SECS: u64 = 60;

#[derive(Debug)]
struct GenerationFailure {
    message: String,
    retryable: bool,
    retry_delay: Option<std::time::Duration>,
}

impl std::fmt::Display for GenerationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GenerationFailure {}

fn generation_failure(message: String, code: Option<&str>) -> anyhow::Error {
    GenerationFailure {
        message,
        retryable: code.is_none_or(|code| {
            matches!(
                code,
                "server_error"
                    | "internal_server_error"
                    | "rate_limit_exceeded"
                    | "resource_unavailable"
                    | "service_unavailable"
                    | "timeout"
                    | "request_timeout"
            )
        }),
        retry_delay: None,
    }
    .into()
}

impl LlmClient {
    async fn send_responses_request(
        &self,
        request: &ResponsesRequest,
        options: &LlmRequestOptions,
    ) -> anyhow::Result<reqwest::Response> {
        let response = self
            .client
            .post(format!("{}/responses", self.api_url))
            .bearer_auth(&self.api_key)
            .json(request)
            .timeout(
                options
                    .request_timeout
                    .unwrap_or(std::time::Duration::from_secs(600)),
            )
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry_delay = retry_after_delay(response.headers());
        let body = response.text().await.unwrap_or_default();
        Err(GenerationFailure {
            message: format!("LLM API error {status}: {body}"),
            retryable: status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status.is_server_error(),
            retry_delay,
        }
        .into())
    }

    async fn wait_to_retry(
        &self,
        error: &anyhow::Error,
        options: &LlmRequestOptions,
        retry_count: usize,
    ) -> bool {
        let max_retries = options.max_rate_limit_retries.or_else(|| {
            (self.service_tier.as_deref() != Some("flex")).then_some(MAX_RATE_LIMIT_RETRIES)
        });
        if max_retries.is_some_and(|max| retry_count >= max) {
            return false;
        }
        let retry_delay = if let Some(failure) = error.downcast_ref::<GenerationFailure>() {
            if !failure.retryable {
                return false;
            }
            failure.retry_delay
        } else if let Some(error) = error.downcast_ref::<reqwest::Error>() {
            if !(error.is_timeout() || error.is_connect() || error.is_body()) {
                return false;
            }
            None
        } else {
            return false;
        };
        let delay = generation_retry_delay(retry_delay, retry_count);
        tracing::warn!(retry = retry_count.saturating_add(1),
            delay_ms = delay.as_millis() as u64, error = %truncate_retry_body(&error.to_string()),
            "LLM generation temporarily failed; retrying");
        tokio::time::sleep(delay).await;
        true
    }
}

fn generation_retry_delay(
    retry_after: Option<std::time::Duration>,
    retry_count: usize,
) -> std::time::Duration {
    retry_after
        .unwrap_or_else(|| exponential_retry_delay(retry_count))
        .min(std::time::Duration::from_secs(MAX_RETRY_DELAY_SECS))
}

fn retry_after_delay(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;

    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(std::time::Duration::from_secs(seconds));
    }

    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(
        retry_at
            .duration_since(std::time::SystemTime::now())
            .unwrap_or_default(),
    )
}

fn exponential_retry_delay(retry_count: usize) -> std::time::Duration {
    let exponent = u32::try_from(retry_count).unwrap_or(u32::MAX).min(5);
    let seconds = BASE_RETRY_DELAY_SECS
        .saturating_mul(2_u64.saturating_pow(exponent))
        .min(MAX_RETRY_DELAY_SECS);
    let jitter_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::from(duration.subsec_millis()))
        .unwrap_or_default();

    std::time::Duration::from_secs(seconds) + std::time::Duration::from_millis(jitter_ms)
}

fn truncate_retry_body(body: &str) -> String {
    const MAX_CHARS: usize = 500;
    let mut chars = body.chars();
    let truncated: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}
