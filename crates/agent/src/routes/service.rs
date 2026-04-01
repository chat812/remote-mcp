use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;

#[derive(Deserialize)]
pub struct ServiceLogsQuery {
    pub tail: Option<usize>,
}

async fn systemctl(args: &[&str]) -> Result<String, String> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    let out = String::from_utf8_lossy(&output.stdout).into_owned();
    let err = String::from_utf8_lossy(&output.stderr).into_owned();

    if output.status.success() {
        Ok(out)
    } else {
        Err(format!("{}{}", out, err))
    }
}

pub async fn get_service_list(State(_state): State<AppState>) -> impl IntoResponse {
    match systemctl(&["list-units", "--type=service", "--no-pager", "--plain", "--output=json"]).await {
        Ok(out) => {
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(&out);
            match parsed {
                Ok(v) => (StatusCode::OK, Json(v)).into_response(),
                Err(_) => (StatusCode::OK, Json(json!({ "output": out }))).into_response(),
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e, "code": "SYSTEMCTL_ERROR" })),
        )
            .into_response(),
    }
}

pub async fn get_service_status(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match systemctl(&["status", &name, "--no-pager"]).await {
        Ok(out) => (StatusCode::OK, Json(json!({ "output": out, "name": name }))).into_response(),
        Err(e) => (
            StatusCode::OK, // status can fail with non-zero for inactive services
            Json(json!({ "output": e, "name": name })),
        )
            .into_response(),
    }
}

pub async fn post_service_start(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match systemctl(&["start", &name]).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "SYSTEMCTL_ERROR" }))).into_response(),
    }
}

pub async fn post_service_stop(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match systemctl(&["stop", &name]).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "SYSTEMCTL_ERROR" }))).into_response(),
    }
}

pub async fn post_service_restart(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match systemctl(&["restart", &name]).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "SYSTEMCTL_ERROR" }))).into_response(),
    }
}

pub async fn post_service_enable(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match systemctl(&["enable", &name]).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "SYSTEMCTL_ERROR" }))).into_response(),
    }
}

pub async fn post_service_disable(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match systemctl(&["disable", &name]).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "SYSTEMCTL_ERROR" }))).into_response(),
    }
}

pub async fn get_service_logs(
    State(_state): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<ServiceLogsQuery>,
) -> impl IntoResponse {
    let n = q.tail.unwrap_or(100).to_string();
    let output = Command::new("journalctl")
        .args(["-u", &name, "-n", &n, "--no-pager", "--output=short"])
        .output()
        .await;

    match output {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).into_owned();
            (StatusCode::OK, Json(json!({ "output": out, "name": name }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string(), "code": "JOURNALCTL_ERROR" }))).into_response(),
    }
}
