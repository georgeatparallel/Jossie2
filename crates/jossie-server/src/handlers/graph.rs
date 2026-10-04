use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};

use crate::errors::AppError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct GraphQuery {
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct GraphResponse {
    pub nodes: Vec<jossie_db::GraphNode>,
    pub edges: Vec<jossie_db::GraphEdge>,
}

pub async fn graph_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<GraphQuery>,
) -> Result<Json<GraphResponse>, AppError> {
    let limit = query.limit.unwrap_or(500);
    let nodes = state.db.graph_list_nodes(limit).await?;
    let edges = state.db.graph_list_edges(limit).await?;

    Ok(Json(GraphResponse { nodes, edges }))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<jossie_db::GraphSearchPage>, AppError> {
    let cursor: Option<(String, String)> = query
        .cursor
        .as_deref()
        .map(jossie_db::decode_graph_cursor)
        .transpose()
        .map_err(|_| AppError::bad_request(anyhow::anyhow!("Invalid graph search cursor")))?;
    Ok(Json(
        state
            .db
            .graph_search_page(
                query.query.as_deref().unwrap_or(""),
                cursor
                    .as_ref()
                    .map(|(label, id)| (label.as_str(), id.as_str())),
                query.limit.unwrap_or(90),
            )
            .await?,
    ))
}

#[derive(Deserialize)]
pub struct NeighborhoodQuery {
    pub node_id: String,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

pub async fn neighborhood(
    State(state): State<Arc<AppState>>,
    Query(query): Query<NeighborhoodQuery>,
) -> Result<Json<jossie_db::GraphNeighborhoodPage>, AppError> {
    let cursor: Option<String> = query
        .cursor
        .as_deref()
        .map(jossie_db::decode_graph_cursor)
        .transpose()
        .map_err(|_| AppError::bad_request(anyhow::anyhow!("Invalid graph neighborhood cursor")))?;
    Ok(Json(
        state
            .db
            .graph_neighborhood_page(
                &query.node_id,
                cursor.as_deref(),
                query.limit.unwrap_or(100),
            )
            .await?
            .ok_or_else(|| AppError::not_found(anyhow::anyhow!("Entity not found")))?,
    ))
}
