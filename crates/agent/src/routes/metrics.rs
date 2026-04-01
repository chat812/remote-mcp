use axum::{extract::State, response::Json};

use super::AppState;

pub async fn get_metrics(State(state): State<AppState>) -> Json<serde_json::Value> {
    let snap = state.metrics.snapshot();
    Json(serde_json::to_value(&snap).unwrap_or_default())
}
