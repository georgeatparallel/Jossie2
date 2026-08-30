use crate::agent::run_agent_loop;
use crate::errors::AppError;
use crate::events::{ServerEvent, persist_message};
use crate::state::AppState;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use jossie_core::types::{Message, Role};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct PendingActionQuery {
    conversation_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionDecisionResponse {
    pub action_id: String,
    pub status: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct ApproveActionQuery {
    revision: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDraftRequest {
    text: String,
    expected_revision: i64,
}

#[derive(Debug, Clone)]
pub struct DeferredActionDecision {
    pub response: ActionDecisionResponse,
    pub conversation_id: Uuid,
    pub batch_id: String,
    pub batch_resolved: bool,
}

pub async fn list_pending_actions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PendingActionQuery>,
) -> Result<Json<Vec<jossie_db::PendingAction>>, AppError> {
    Ok(Json(
        state.db.list_pending_actions(query.conversation_id).await?,
    ))
}

fn resume_batch_when_ready(state: Arc<AppState>, batch_id: String, conversation_id: Uuid) {
    tokio::spawn(async move {
        match state.db.pending_action_batch_is_resolved(&batch_id).await {
            Ok(true) => {
                if let Err(error) = run_agent_loop(&state, conversation_id).await {
                    tracing::error!(
                        "Failed to resume conversation {conversation_id} after action batch {batch_id}: {error}"
                    );
                }
            }
            Ok(false) => {}
            Err(error) => tracing::error!("Failed to inspect action batch {batch_id}: {error}"),
        }
    });
}

pub async fn approve_action(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<ApproveActionQuery>,
) -> Result<Json<ActionDecisionResponse>, AppError> {
    decide_action_at_revision(state, id, true, query.revision)
        .await
        .map(Json)
}

pub async fn reject_action(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ActionDecisionResponse>, AppError> {
    decide_action(state, id, false).await.map(Json)
}

pub async fn decide_action(
    state: Arc<AppState>,
    id: String,
    approve: bool,
) -> Result<ActionDecisionResponse, AppError> {
    decide_action_at_revision(state, id, approve, None).await
}

pub async fn decide_action_at_revision(
    state: Arc<AppState>,
    id: String,
    approve: bool,
    expected_revision: Option<i64>,
) -> Result<ActionDecisionResponse, AppError> {
    let outcome =
        decide_action_deferred_at_revision(state.clone(), id, approve, expected_revision).await?;
    if outcome.batch_resolved {
        resume_batch_when_ready(state, outcome.batch_id, outcome.conversation_id);
    }
    Ok(outcome.response)
}

pub async fn decide_action_deferred(
    state: Arc<AppState>,
    id: String,
    approve: bool,
) -> Result<DeferredActionDecision, AppError> {
    decide_action_deferred_at_revision(state, id, approve, None).await
}

pub async fn decide_action_deferred_at_revision(
    state: Arc<AppState>,
    id: String,
    approve: bool,
    expected_revision: Option<i64>,
) -> Result<DeferredActionDecision, AppError> {
    let action = match state
        .db
        .claim_pending_action_at_revision(&id, expected_revision)
        .await?
    {
        Some(action) => action,
        None => {
            let existing = state
                .db
                .get_pending_action(&id)
                .await?
                .ok_or_else(|| AppError::not_found(anyhow::anyhow!("Action not found")))?;
            if expected_revision.is_some_and(|revision| revision != existing.revision) {
                return Err(AppError::conflict(anyhow::anyhow!(
                    "The draft changed; review the latest revision before sending"
                )));
            }
            if matches!(
                existing.status.as_str(),
                "completed" | "failed" | "rejected" | "uncertain"
            ) {
                return Ok(DeferredActionDecision {
                    response: ActionDecisionResponse {
                        action_id: existing.id,
                        status: existing.status,
                    },
                    conversation_id: existing.conversation_id,
                    batch_id: existing.batch_id,
                    // This request did not resolve anything, so callers must not
                    // resume an already-finished batch a second time.
                    batch_resolved: false,
                });
            }
            return Err(AppError::conflict(anyhow::anyhow!(
                "Action is already executing"
            )));
        }
    };

    let terminal_status = if approve && action.kind == "telegram_secretary_reply" {
        match execute_secretary_reply(&state, &action).await {
            Ok(status) => status,
            Err(error) => {
                let message = error.to_string();
                state
                    .db
                    .resolve_pending_action(&action.id, "failed", Some(&message))
                    .await?;
                "failed"
            }
        }
    } else if approve {
        let call = jossie_core::ToolCall {
            id: action.call_id.clone(),
            name: action.tool_name.clone(),
            arguments: action.arguments.clone(),
        };
        state
            .publish_event(ServerEvent::ToolStarted {
                conversation_id: action.conversation_id,
                run_id: action.run_id.clone(),
                call_id: action.call_id.clone(),
                tool: action.tool_name.clone(),
            })
            .await;
        let result = state.registry.execute(&call).await;
        let status = if result.is_error {
            "failed"
        } else {
            "completed"
        };
        let error = result.is_error.then_some(result.content.as_str());
        let tool_message = Message::new(action.conversation_id, Role::Tool, result.content.clone())
            .with_tool_call_id(action.call_id.clone())
            .with_name(action.tool_name.clone());
        persist_message(&state, &tool_message).await?;
        state.db.resolve_pending_action(&id, status, error).await?;
        state
            .publish_event(ServerEvent::ToolFinished {
                conversation_id: action.conversation_id,
                run_id: action.run_id.clone(),
                call_id: action.call_id.clone(),
                tool: action.tool_name.clone(),
                result_preview: crate::events::preview_text(&result.content, 220),
                is_error: result.is_error,
            })
            .await;
        status
    } else {
        let output = "The user did not approve this action. Do not attempt it again unless they make a new explicit request.";
        let tool_message = Message::new(action.conversation_id, Role::Tool, output.to_string())
            .with_tool_call_id(action.call_id.clone())
            .with_name(action.tool_name.clone());
        persist_message(&state, &tool_message).await?;
        state
            .db
            .resolve_pending_action(&id, "rejected", None)
            .await?;
        "rejected"
    };

    state
        .publish_event(ServerEvent::ActionResolved {
            conversation_id: action.conversation_id,
            run_id: action.run_id.clone(),
            action_id: action.id.clone(),
            status: terminal_status.to_string(),
            title: if action.kind == "telegram_secretary_reply" {
                "Telegram secretary reply".to_string()
            } else {
                action.title.clone()
            },
        })
        .await;
    let batch_resolved = action.resume_on_resolve
        && state
            .db
            .pending_action_batch_is_resolved(&action.batch_id)
            .await?;

    Ok(DeferredActionDecision {
        response: ActionDecisionResponse {
            action_id: action.id,
            status: terminal_status.to_string(),
        },
        conversation_id: action.conversation_id,
        batch_id: action.batch_id,
        batch_resolved,
    })
}

pub async fn update_secretary_draft(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<UpdateDraftRequest>,
) -> Result<Json<jossie_db::PendingAction>, AppError> {
    let updated = state
        .db
        .update_secretary_draft(&id, &request.text, request.expected_revision)
        .await?
        .ok_or_else(|| {
            AppError::conflict(anyhow::anyhow!("The draft changed or is no longer pending"))
        })?;
    state
        .publish_event(ServerEvent::ActionApprovalRequired {
            conversation_id: updated.conversation_id,
            run_id: updated.run_id.clone(),
            action: updated.clone(),
        })
        .await;
    Ok(Json(updated))
}

#[derive(Deserialize)]
struct SecretaryReplyArguments {
    business_connection_id: String,
    telegram_chat_id: i64,
    reply_to_message_id: Option<i32>,
}

async fn execute_secretary_reply(
    state: &Arc<AppState>,
    action: &jossie_db::PendingAction,
) -> Result<&'static str, AppError> {
    if !state.telegram.secretary_enabled {
        return Err(AppError::conflict(anyhow::anyhow!(
            "Telegram secretary is disabled"
        )));
    }
    let args: SecretaryReplyArguments = serde_json::from_str(&action.arguments)
        .map_err(|error| AppError::from(anyhow::anyhow!(error)))?;
    let chat = state
        .db
        .get_telegram_business_chat_for_conversation(action.conversation_id)
        .await?
        .ok_or_else(|| AppError::conflict(anyhow::anyhow!("Managed Telegram chat is gone")))?;
    if chat.connection_id != args.business_connection_id
        || chat.telegram_chat_id != args.telegram_chat_id
    {
        return Err(AppError::conflict(anyhow::anyhow!(
            "Managed Telegram chat routing changed"
        )));
    }
    if action.source_revision != Some(chat.transcript_revision) {
        return Err(AppError::conflict(anyhow::anyhow!(
            "A newer Telegram message arrived; review a fresh draft"
        )));
    }
    let connection = state
        .db
        .get_telegram_business_connection(&chat.connection_id)
        .await?
        .filter(|connection| connection.is_enabled)
        .ok_or_else(|| AppError::conflict(anyhow::anyhow!("Secretary connection is disabled")))?;
    if state.telegram.allowed_user_id != Some(connection.owner_user_id) {
        return Err(AppError::conflict(anyhow::anyhow!(
            "Secretary connection does not belong to the configured owner"
        )));
    }
    if !connection
        .rights
        .get("can_reply")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err(AppError::conflict(anyhow::anyhow!(
            "Secretary connection no longer has permission to reply"
        )));
    }
    let last_inbound = chat
        .last_inbound_at
        .as_deref()
        .and_then(|value| value.parse::<chrono::DateTime<chrono::Utc>>().ok())
        .ok_or_else(|| AppError::conflict(anyhow::anyhow!("No eligible incoming message")))?;
    if chrono::Utc::now() - last_inbound > chrono::Duration::hours(24) {
        return Err(AppError::conflict(anyhow::anyhow!(
            "Telegram's 24-hour reply window has expired"
        )));
    }
    let text = action.editable_text.as_deref().unwrap_or_default().trim();
    if text.is_empty() || text.chars().count() > 4096 {
        return Err(AppError::conflict(anyhow::anyhow!("Draft text is invalid")));
    }
    let sender =
        state.telegram.secretary_sender.as_ref().ok_or_else(|| {
            AppError::conflict(anyhow::anyhow!("Telegram secretary is unavailable"))
        })?;
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    sender
        .send(crate::state::TelegramSecretaryCommand {
            business_connection_id: chat.connection_id.clone(),
            chat_id: chat.telegram_chat_id,
            reply_to_message_id: args.reply_to_message_id,
            text: text.to_string(),
            response: response_tx,
        })
        .await
        .map_err(|_| AppError::from(anyhow::anyhow!("Telegram sender stopped")))?;
    let sent = response_rx
        .await
        .map_err(|_| AppError::from(anyhow::anyhow!("Telegram sender dropped response")))?
        .map_err(AppError::from)?;
    let message = Message::new(action.conversation_id, Role::Assistant, text.to_string())
        .with_name("telegram_secretary_owner".to_string());
    persist_message(state, &message).await?;
    state
        .db
        .record_telegram_business_message(
            &chat.connection_id,
            chat.telegram_chat_id,
            sent.message_id,
            message.id,
            "outbound",
            &sent.sent_at.to_rfc3339(),
        )
        .await?;
    state
        .db
        .resolve_pending_action(&action.id, "completed", None)
        .await?;
    Ok("completed")
}
