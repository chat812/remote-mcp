use crate::routes::AppState;
use crate::sessions::Session;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct SessionOpenRequest {
    pub workdir: Option<String>,
    pub shell: Option<String>,
}

#[derive(Deserialize)]
pub struct SessionExecRequest {
    pub command: String,
    pub timeout_secs: Option<u64>,
}

pub async fn post_session_open(
    State(state): State<AppState>,
    Json(req): Json<SessionOpenRequest>,
) -> impl IntoResponse {
    match Session::open(req.workdir, req.shell) {
        Ok(session) => {
            let id = session.id.clone();
            state.sessions.insert(id.clone(), session);
            state.metrics.inc_sessions();
            (StatusCode::OK, Json(json!({ "session_id": id }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string(), "code": "SESSION_OPEN_ERROR" })),
        )
            .into_response(),
    }
}

pub async fn post_session_exec(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SessionExecRequest>,
) -> impl IntoResponse {
    match state.sessions.get(&id) {
        Some(session) => {
            let timeout = req.timeout_secs.unwrap_or(30);
            match session.exec(&req.command, timeout).await {
                Ok((stdout, exit_code)) => (
                    StatusCode::OK,
                    Json(json!({
                        "stdout": stdout,
                        "exit_code": exit_code,
                    })),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string(), "code": "SESSION_EXEC_ERROR" })),
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "session not found", "code": "SESSION_NOT_FOUND" })),
        )
            .into_response(),
    }
}

pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.sessions.remove(&id).is_some() {
        state.metrics.dec_sessions();
        (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "session not found", "code": "SESSION_NOT_FOUND" })),
        )
            .into_response()
    }
}

pub async fn get_sessions(State(state): State<AppState>) -> impl IntoResponse {
    let sessions: Vec<_> = state
        .sessions
        .iter()
        .map(|entry| {
            let session = entry.value();
            json!({
                "id": session.id,
                "cwd": session.cwd,
                "idle_secs": session.idle_secs(),
            })
        })
        .collect();

    Json(json!({ "sessions": sessions }))
}
