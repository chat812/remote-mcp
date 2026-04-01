use crate::routes::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sysinfo::System;

#[derive(Deserialize)]
pub struct ProcessListQuery {
    pub filter: Option<String>,
}

#[derive(Deserialize)]
pub struct ProcessTreeQuery {
    pub pid: Option<u32>,
}

#[derive(Deserialize)]
pub struct KillRequest {
    pub pid: u32,
    pub signal: Option<String>,
}

pub async fn get_process_list(
    State(_state): State<AppState>,
    Query(q): Query<ProcessListQuery>,
) -> impl IntoResponse {
    let mut sys = System::new_all();
    sys.refresh_all();

    let processes: Vec<_> = sys
        .processes()
        .iter()
        .filter(|(_, proc)| {
            if let Some(filter) = &q.filter {
                proc.name().contains(filter.as_str())
                    || proc.cmd().iter().any(|s| s.contains(filter.as_str()))
            } else {
                true
            }
        })
        .map(|(pid, proc)| {
            json!({
                "pid": pid.as_u32(),
                "name": proc.name(),
                "cmd": proc.cmd().join(" "),
                "cpu_usage": proc.cpu_usage(),
                "memory_kb": proc.memory(),
                "status": format!("{:?}", proc.status()),
                "parent": proc.parent().map(|p| p.as_u32()),
            })
        })
        .collect();

    (StatusCode::OK, Json(json!({ "processes": processes }))).into_response()
}

pub async fn post_process_kill(
    State(_state): State<AppState>,
    Json(req): Json<KillRequest>,
) -> impl IntoResponse {
    #[cfg(unix)]
    {
        use nix::sys::signal::{self, Signal};
        use nix::unistd::Pid;
        use std::str::FromStr;

        let signal = match req.signal.as_deref().unwrap_or("TERM") {
            "TERM" | "SIGTERM" | "15" => Signal::SIGTERM,
            "KILL" | "SIGKILL" | "9" => Signal::SIGKILL,
            "HUP" | "SIGHUP" | "1" => Signal::SIGHUP,
            "INT" | "SIGINT" | "2" => Signal::SIGINT,
            _ => Signal::SIGTERM,
        };

        match signal::kill(Pid::from_raw(req.pid as i32), signal) {
            Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string(), "code": "KILL_ERROR" })),
            )
                .into_response(),
        }
    }

    #[cfg(not(unix))]
    {
        // Windows: use taskkill
        let output = tokio::process::Command::new("taskkill")
            .args(["/F", "/PID", &req.pid.to_string()])
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
            Ok(o) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": String::from_utf8_lossy(&o.stderr).to_string(), "code": "KILL_ERROR" })),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string(), "code": "KILL_ERROR" })),
            )
                .into_response(),
        }
    }
}

pub async fn get_process_tree(
    State(_state): State<AppState>,
    Query(q): Query<ProcessTreeQuery>,
) -> impl IntoResponse {
    let mut sys = System::new_all();
    sys.refresh_all();

    if let Some(root_pid) = q.pid {
        let tree = build_process_tree(&sys, root_pid);
        (StatusCode::OK, Json(json!({ "tree": tree }))).into_response()
    } else {
        // Return top-level processes (no parent or parent not in list)
        let all_pids: std::collections::HashSet<u32> = sys.processes().keys().map(|p| p.as_u32()).collect();
        let roots: Vec<_> = sys
            .processes()
            .iter()
            .filter(|(_, proc)| {
                proc.parent().map(|p| !all_pids.contains(&p.as_u32())).unwrap_or(true)
            })
            .map(|(pid, proc)| {
                build_process_tree(&sys, pid.as_u32())
            })
            .collect();
        (StatusCode::OK, Json(json!({ "tree": roots }))).into_response()
    }
}

fn build_process_tree(sys: &System, pid: u32) -> serde_json::Value {
    use sysinfo::Pid;
    let spid = sysinfo::Pid::from_u32(pid);

    if let Some(proc) = sys.process(spid) {
        let children: Vec<_> = sys
            .processes()
            .iter()
            .filter(|(_, p)| p.parent() == Some(spid))
            .map(|(cpid, _)| build_process_tree(sys, cpid.as_u32()))
            .collect();

        json!({
            "pid": pid,
            "name": proc.name(),
            "children": children,
        })
    } else {
        json!({ "pid": pid, "name": "unknown", "children": [] })
    }
}
