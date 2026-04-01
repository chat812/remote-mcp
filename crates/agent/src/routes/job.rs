use crate::jobs::{self, JobStatus};
use crate::routes::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize)]
pub struct JobStartRequest {
    pub command: String,
    pub workdir: Option<String>,
}

#[derive(Serialize)]
pub struct JobStartResponse {
    pub job_id: String,
}

#[derive(Deserialize)]
pub struct JobLogsQuery {
    pub tail: Option<usize>,
    pub stream: Option<String>,
}

pub async fn post_job_start(
    State(state): State<AppState>,
    Json(req): Json<JobStartRequest>,
) -> impl IntoResponse {
    let hot = state.config.get_hot();
    if state.jobs.len() >= hot.max_jobs {
        state.metrics.inc_rejected();
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "max jobs reached", "code": "TOO_MANY_JOBS" })),
        )
            .into_response();
    }

    match jobs::start_job(&state.jobs, req.command, req.workdir).await {
        Ok(job) => {
            state.metrics.inc_jobs_started();
            let id = job.id.clone();
            // Track when job finishes
            let metrics = state.metrics.clone();
            let notify = job.done_notify.clone();
            tokio::spawn(async move {
                notify.notified().await;
                metrics.dec_jobs_running();
            });
            (StatusCode::OK, Json(json!({ "job_id": id }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string(), "code": "JOB_START_ERROR" })),
        )
            .into_response(),
    }
}

pub async fn get_job_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.jobs.get(&id) {
        Some(job) => {
            let status = job.get_status();
            let exit_code = *job.exit_code.lock().unwrap();
            let finished_at = *job.finished_at.lock().unwrap();
            (
                StatusCode::OK,
                Json(json!({
                    "job_id": job.id,
                    "command": job.command,
                    "status": status,
                    "exit_code": exit_code,
                    "started_at": job.started_at,
                    "finished_at": finished_at,
                })),
            )
                .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "job not found", "code": "JOB_NOT_FOUND" })),
        )
            .into_response(),
    }
}

pub async fn get_job_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<JobLogsQuery>,
) -> impl IntoResponse {
    match state.jobs.get(&id) {
        Some(job) => {
            let stream = q.stream.as_deref().unwrap_or("both");
            let tail = q.tail.unwrap_or(1000);

            let stdout = if stream == "both" || stream == "stdout" {
                let lines = job.stdout.tail(tail);
                Some(lines.join("\n"))
            } else {
                None
            };

            let stderr = if stream == "both" || stream == "stderr" {
                let lines = job.stderr.tail(tail);
                Some(lines.join("\n"))
            } else {
                None
            };

            (
                StatusCode::OK,
                Json(json!({
                    "job_id": id,
                    "stdout": stdout,
                    "stderr": stderr,
                    "bytes_dropped_stdout": job.stdout.bytes_dropped.load(std::sync::atomic::Ordering::Relaxed),
                    "bytes_dropped_stderr": job.stderr.bytes_dropped.load(std::sync::atomic::Ordering::Relaxed),
                })),
            )
                .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "job not found", "code": "JOB_NOT_FOUND" })),
        )
            .into_response(),
    }
}

pub async fn post_job_kill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.jobs.get(&id) {
        Some(job) => match jobs::kill_job(&job).await {
            Ok(_) => {
                (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string(), "code": "KILL_ERROR" })),
            )
                .into_response(),
        },
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "job not found", "code": "JOB_NOT_FOUND" })),
        )
            .into_response(),
    }
}

pub async fn get_jobs(State(state): State<AppState>) -> impl IntoResponse {
    let jobs: Vec<_> = state
        .jobs
        .iter()
        .map(|entry| {
            let job = entry.value();
            let status = job.get_status();
            let exit_code = *job.exit_code.lock().unwrap();
            json!({
                "job_id": job.id,
                "command": job.command,
                "status": status,
                "exit_code": exit_code,
                "started_at": job.started_at,
            })
        })
        .collect();

    Json(json!({ "jobs": jobs }))
}
