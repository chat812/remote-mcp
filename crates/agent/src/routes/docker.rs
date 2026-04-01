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
pub struct DockerPsQuery {
    pub all: Option<bool>,
}

#[derive(Deserialize)]
pub struct DockerLogsQuery {
    pub tail: Option<usize>,
}

#[derive(Deserialize)]
pub struct DockerExecRequest {
    pub command: String,
}

async fn docker_cmd(args: &[&str]) -> Result<String, String> {
    let output = Command::new("docker")
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

pub async fn get_docker_ps(
    State(_state): State<AppState>,
    Query(q): Query<DockerPsQuery>,
) -> impl IntoResponse {
    let mut args = vec!["ps", "--format", "{{json .}}"];
    if q.all.unwrap_or(false) {
        args.push("-a");
    }

    match docker_cmd(&args).await {
        Ok(out) => {
            let containers: Vec<serde_json::Value> = out
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            (StatusCode::OK, Json(json!({ "containers": containers }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "DOCKER_ERROR" }))).into_response(),
    }
}

pub async fn get_docker_logs(
    State(_state): State<AppState>,
    Path(container): Path<String>,
    Query(q): Query<DockerLogsQuery>,
) -> impl IntoResponse {
    let n = q.tail.unwrap_or(100).to_string();
    match docker_cmd(&["logs", "--tail", &n, &container]).await {
        Ok(out) => (StatusCode::OK, Json(json!({ "output": out }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "DOCKER_ERROR" }))).into_response(),
    }
}

pub async fn post_docker_exec(
    State(_state): State<AppState>,
    Path(container): Path<String>,
    Json(req): Json<DockerExecRequest>,
) -> impl IntoResponse {
    let output = Command::new("docker")
        .args(["exec", &container, "sh", "-c", &req.command])
        .output()
        .await;

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
            let exit_code = o.status.code().unwrap_or(-1);
            (StatusCode::OK, Json(json!({ "stdout": stdout, "stderr": stderr, "exit_code": exit_code }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string(), "code": "DOCKER_ERROR" }))).into_response(),
    }
}

async fn docker_action_route(container: &str, action: &str) -> impl IntoResponse {
    match docker_cmd(&[action, container]).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "DOCKER_ERROR" }))).into_response(),
    }
}

pub async fn post_docker_start(
    State(_state): State<AppState>,
    Path(container): Path<String>,
) -> impl IntoResponse {
    docker_action_route(&container, "start").await
}

pub async fn post_docker_stop(
    State(_state): State<AppState>,
    Path(container): Path<String>,
) -> impl IntoResponse {
    docker_action_route(&container, "stop").await
}

pub async fn post_docker_restart(
    State(_state): State<AppState>,
    Path(container): Path<String>,
) -> impl IntoResponse {
    docker_action_route(&container, "restart").await
}

pub async fn get_docker_inspect(
    State(_state): State<AppState>,
    Path(container): Path<String>,
) -> impl IntoResponse {
    match docker_cmd(&["inspect", "--format", "{{json .}}", &container]).await {
        Ok(out) => {
            let parsed: serde_json::Value = serde_json::from_str(&out).unwrap_or(json!({ "output": out }));
            (StatusCode::OK, Json(parsed)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "DOCKER_ERROR" }))).into_response(),
    }
}

pub async fn get_docker_images(State(_state): State<AppState>) -> impl IntoResponse {
    match docker_cmd(&["images", "--format", "{{json .}}"]).await {
        Ok(out) => {
            let images: Vec<serde_json::Value> = out
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            (StatusCode::OK, Json(json!({ "images": images }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e, "code": "DOCKER_ERROR" }))).into_response(),
    }
}
