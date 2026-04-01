use crate::routes::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

#[derive(Deserialize)]
pub struct ExecRequest {
    pub command: String,
    pub workdir: Option<String>,
    pub timeout_secs: Option<u64>,
    pub env: Option<std::collections::HashMap<String, String>>,
}

#[derive(Serialize)]
pub struct ExecResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

pub async fn post_exec(
    State(state): State<AppState>,
    Json(req): Json<ExecRequest>,
) -> impl IntoResponse {
    let _permit = match state.exec_semaphore.try_acquire() {
        Ok(p) => p,
        Err(_) => {
            state.metrics.inc_rejected();
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({ "error": "too many concurrent execs", "code": "TOO_MANY_REQUESTS" })),
            )
                .into_response();
        }
    };

    state.metrics.inc_execs();
    let start = std::time::Instant::now();
    let timeout_secs = req.timeout_secs.unwrap_or(120).min(3600);

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", &req.command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &req.command]);
        c
    };

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    if let Some(wd) = &req.workdir {
        cmd.current_dir(wd);
    }

    if let Some(env) = &req.env {
        for (k, v) in env {
            cmd.env(k, v);
        }
    }

    let result = timeout(Duration::from_secs(timeout_secs), async {
        let output = cmd.output().await?;
        Ok::<_, std::io::Error>(output)
    })
    .await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(output)) => {
            let exit_code = output.status.code().unwrap_or(-1);
            if exit_code != 0 {
                state.metrics.inc_exec_errors();
            }
            (
                StatusCode::OK,
                Json(json!(ExecResponse {
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    exit_code,
                    duration_ms,
                })),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            state.metrics.inc_exec_errors();
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string(), "code": "EXEC_ERROR" })),
            )
                .into_response()
        }
        Err(_) => {
            state.metrics.inc_exec_errors();
            (
                StatusCode::REQUEST_TIMEOUT,
                Json(json!({ "error": format!("timeout after {}s", timeout_secs), "code": "TIMEOUT" })),
            )
                .into_response()
        }
    }
}
