use crate::{errors::AppError, state::AppState};
use axum::{Json, extract::State};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct SecretaryStatus {
    enabled: bool,
    encryption_ready: bool,
    owner_user_id: Option<i64>,
    connections: Vec<jossie_db::TelegramBusinessConnection>,
    retention: &'static str,
    approval_policy: &'static str,
}

pub async fn status(State(state): State<Arc<AppState>>) -> Result<Json<SecretaryStatus>, AppError> {
    Ok(Json(SecretaryStatus {
        enabled: state.telegram.secretary_enabled,
        encryption_ready: state.db.sensitive_data_encryption_enabled(),
        owner_user_id: state.telegram.allowed_user_id,
        connections: state.db.list_telegram_business_connections().await?,
        retention: "Managed chat data is retained until the chat is deleted or the secretary connection is disabled.",
        approval_policy: "Every outbound reply requires owner approval of the exact text.",
    }))
}
