pub mod docker;
pub mod exec;
pub mod file;
pub mod fs;
pub mod git;
pub mod job;
pub mod logs;
pub mod metrics;
pub mod process;
pub mod service;
pub mod session;
pub mod sysinfo;
pub mod ui;

use crate::capabilities::Capabilities;
use crate::config::Config;
use crate::jobs::JobStore;
use crate::metrics::Metrics;
use crate::sessions::SessionStore;
use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub jobs: JobStore,
    pub sessions: SessionStore,
    pub metrics: Arc<Metrics>,
    pub capabilities: Arc<Capabilities>,
    pub exec_semaphore: Arc<Semaphore>,
    pub file_semaphore: Arc<Semaphore>,
}

pub fn build_router(state: AppState) -> Router {
    let token = state.config.token.clone();
    let config_for_auth = state.config.clone();

    Router::new()
        // Public routes
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics::get_metrics))
        .route("/capabilities", get(get_capabilities))
        // Protected routes
        .route("/exec", post(exec::post_exec))
        .route("/job/start", post(job::post_job_start))
        .route("/job/:id", get(job::get_job_status))
        .route("/job/:id/logs", get(job::get_job_logs))
        .route("/job/:id/kill", post(job::post_job_kill))
        .route("/jobs", get(job::get_jobs))
        .route("/file/upload", post(file::post_file_upload))
        .route("/file/download", get(file::get_file_download))
        .route("/file/write", post(file::post_file_write))
        .route("/file/read", get(file::get_file_read))
        .route("/file/str-replace", post(file::post_file_str_replace))
        .route("/file/patch", post(file::post_file_patch))
        .route("/file/insert", post(file::post_file_insert))
        .route("/file/delete-lines", post(file::post_file_delete_lines))
        .route("/fs/ls", get(fs::get_ls))
        .route("/fs/stat", get(fs::get_stat))
        .route("/fs/find", post(fs::post_find))
        .route("/fs/tree", get(fs::get_tree))
        .route("/fs/mkdir", post(fs::post_mkdir))
        .route("/fs/rm", delete(fs::delete_rm))
        .route("/fs/mv", post(fs::post_mv))
        .route("/fs/cp", post(fs::post_cp))
        .route("/process/list", get(process::get_process_list))
        .route("/process/kill", post(process::post_process_kill))
        .route("/process/tree", get(process::get_process_tree))
        .route("/service/list", get(service::get_service_list))
        .route("/service/:name/status", get(service::get_service_status))
        .route("/service/:name/start", post(service::post_service_start))
        .route("/service/:name/stop", post(service::post_service_stop))
        .route("/service/:name/restart", post(service::post_service_restart))
        .route("/service/:name/enable", post(service::post_service_enable))
        .route("/service/:name/disable", post(service::post_service_disable))
        .route("/service/:name/logs", get(service::get_service_logs))
        .route("/log/tail", get(logs::get_log_tail))
        .route("/log/grep", get(logs::get_log_grep))
        .route("/sysinfo", get(sysinfo::get_sysinfo))
        .route("/sysinfo/disk", get(sysinfo::get_disk))
        .route("/sysinfo/ports", get(sysinfo::get_ports))
        .route("/sysinfo/ping", post(sysinfo::post_ping))
        .route("/sysinfo/interfaces", get(sysinfo::get_interfaces))
        .route("/session", post(session::post_session_open))
        .route("/session/:id/exec", post(session::post_session_exec))
        .route("/session/:id", delete(session::delete_session))
        .route("/sessions", get(session::get_sessions))
        .route("/docker/ps", get(docker::get_docker_ps))
        .route("/docker/:container/logs", get(docker::get_docker_logs))
        .route("/docker/:container/exec", post(docker::post_docker_exec))
        .route("/docker/:container/start", post(docker::post_docker_start))
        .route("/docker/:container/stop", post(docker::post_docker_stop))
        .route("/docker/:container/restart", post(docker::post_docker_restart))
        .route("/docker/:container/inspect", get(docker::get_docker_inspect))
        .route("/docker/images", get(docker::get_docker_images))
        .route("/git/status", get(git::get_git_status))
        .route("/git/log", get(git::get_git_log))
        .route("/git/diff", get(git::get_git_diff))
        .route("/git/pull", post(git::post_git_pull))
        .route("/git/checkout", post(git::post_git_checkout))
        // UI automation routes (Windows only; return 501 on other platforms)
        .route("/ui/windows", get(ui::get_ui_windows))
        .route("/ui/tree", get(ui::get_ui_tree))
        .route("/ui/focus", post(ui::post_ui_focus))
        .route("/ui/click", post(ui::post_ui_click))
        .route("/ui/move", post(ui::post_ui_move))
        .route("/ui/type", post(ui::post_ui_type))
        .route("/ui/key", post(ui::post_ui_key))
        .route("/ui/scroll", post(ui::post_ui_scroll))
        .route("/ui/element", get(ui::get_ui_element))
        .route("/ui/click-element", post(ui::post_ui_click_element))
        .route("/ui/get-value", get(ui::get_ui_value))
        .route("/ui/set-value", post(ui::post_ui_set_value))
        .route("/ui/screenshot", get(ui::get_ui_screenshot))
        .route("/ui/describe", get(ui::get_ui_describe))
        .route("/ui/ocr", get(ui::get_ui_ocr))
        .route_layer(middleware::from_fn(move |req, next| {
            let token = token.clone();
            let allowed_ips = config_for_auth.get_hot().allowed_ips.clone();
            async move { crate::auth::auth_middleware(token, allowed_ips, req, next).await }
        }))
        .with_state(state)
}

async fn health_handler() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn get_capabilities(State(state): State<AppState>) -> impl IntoResponse {
    Json((*state.capabilities).clone())
}

pub fn error_response(status: StatusCode, code: &str, message: &str) -> impl IntoResponse {
    (status, Json(json!({ "error": message, "code": code })))
}
