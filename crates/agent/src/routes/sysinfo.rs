use crate::routes::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use serde_json::json;
use sysinfo::{Disks, Networks, System};
use tokio::process::Command;

#[derive(Deserialize)]
pub struct DiskQuery {
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct PingRequest {
    pub target: String,
    pub count: Option<u32>,
}

pub async fn get_sysinfo(State(_state): State<AppState>) -> impl IntoResponse {
    let mut sys = System::new_all();
    sys.refresh_all();

    let hostname = System::host_name().unwrap_or_else(|| "unknown".to_string());
    let os_name = System::name().unwrap_or_else(|| "unknown".to_string());
    let os_version = System::os_version().unwrap_or_else(|| "unknown".to_string());
    let kernel = System::kernel_version().unwrap_or_else(|| "unknown".to_string());
    let uptime = System::uptime();
    let load = System::load_average();
    let total_mem = sys.total_memory();
    let used_mem = sys.used_memory();
    let total_swap = sys.total_swap();
    let used_swap = sys.used_swap();
    let cpu_count = sys.cpus().len();

    (StatusCode::OK, Json(json!({
        "hostname": hostname,
        "os": os_name,
        "os_version": os_version,
        "kernel": kernel,
        "uptime_secs": uptime,
        "load_avg": {
            "one": load.one,
            "five": load.five,
            "fifteen": load.fifteen,
        },
        "memory": {
            "total_kb": total_mem,
            "used_kb": used_mem,
            "free_kb": total_mem.saturating_sub(used_mem),
        },
        "swap": {
            "total_kb": total_swap,
            "used_kb": used_swap,
        },
        "cpu_count": cpu_count,
        "arch": std::env::consts::ARCH,
    }))).into_response()
}

pub async fn get_disk(
    State(_state): State<AppState>,
    Query(q): Query<DiskQuery>,
) -> impl IntoResponse {
    let disks_list = Disks::new_with_refreshed_list();

    let disks: Vec<_> = disks_list
        .iter()
        .filter(|d| {
            if let Some(path) = &q.path {
                d.mount_point().to_string_lossy().starts_with(path.as_str())
            } else {
                true
            }
        })
        .map(|d| {
            json!({
                "name": d.name().to_string_lossy(),
                "mount": d.mount_point().to_string_lossy(),
                "total_bytes": d.total_space(),
                "available_bytes": d.available_space(),
                "used_bytes": d.total_space().saturating_sub(d.available_space()),
                "fs": d.file_system().to_string_lossy(),
            })
        })
        .collect();

    (StatusCode::OK, Json(json!({ "disks": disks }))).into_response()
}

pub async fn get_ports(State(_state): State<AppState>) -> impl IntoResponse {
    let output = Command::new("sh")
        .args(["-c", "ss -tlnp 2>/dev/null || netstat -tlnp 2>/dev/null"])
        .output()
        .await;

    match output {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).into_owned();
            (StatusCode::OK, Json(json!({ "output": out }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string(), "code": "PORTS_ERROR" })),
        )
            .into_response(),
    }
}

pub async fn post_ping(
    State(_state): State<AppState>,
    Json(req): Json<PingRequest>,
) -> impl IntoResponse {
    let count = req.count.unwrap_or(4).to_string();
    let (cmd, args) = if cfg!(windows) {
        ("ping", vec!["-n", &count, &req.target])
    } else {
        ("ping", vec!["-c", &count, &req.target])
    };

    let output = Command::new(cmd).args(args).output().await;

    match output {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).into_owned();
            let err = String::from_utf8_lossy(&o.stderr).into_owned();
            let success = o.status.success();
            (StatusCode::OK, Json(json!({
                "output": out + &err,
                "success": success,
                "target": req.target,
            }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string(), "code": "PING_ERROR" })),
        )
            .into_response(),
    }
}

pub async fn get_interfaces(State(_state): State<AppState>) -> impl IntoResponse {
    let networks = Networks::new_with_refreshed_list();

    let interfaces: Vec<_> = networks
        .iter()
        .map(|(name, data)| {
            json!({
                "name": name,
                "received_bytes": data.total_received(),
                "transmitted_bytes": data.total_transmitted(),
            })
        })
        .collect();

    (StatusCode::OK, Json(json!({ "interfaces": interfaces }))).into_response()
}
