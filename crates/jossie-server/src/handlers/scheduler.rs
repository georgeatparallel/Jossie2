use crate::{errors::AppError, events::ServerEvent, state::AppState};
use axum::{
    Json,
    extract::{Path, State},
};
use std::sync::Arc;

pub async fn cancel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<jossie_db::ScheduledTask>, AppError> {
    let changed = state.db.cancel_active_scheduled_task(&id).await?;
    let task = state
        .db
        .get_scheduled_task(&id)
        .await?
        .ok_or_else(|| AppError::not_found(anyhow::anyhow!("Scheduled task not found")))?;
    if task.status != "cancelled" {
        return Err(AppError::conflict(anyhow::anyhow!(
            "This schedule is no longer active"
        )));
    }
    if changed {
        state
            .publish_event(ServerEvent::ScheduledTaskUpdated { task: task.clone() })
            .await;
    }
    Ok(Json(task))
}
