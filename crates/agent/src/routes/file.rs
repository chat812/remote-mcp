use crate::routes::AppState;
use axum::{
    body::Body,
    extract::{Multipart, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(Deserialize)]
pub struct PathQuery {
    pub path: String,
}

#[derive(Deserialize)]
pub struct FileWriteRequest {
    pub path: String,
    pub content: String,
    pub mode: Option<String>,
}

#[derive(Deserialize)]
pub struct FileStrReplaceRequest {
    pub path: String,
    pub old_str: String,
    pub new_str: String,
}

#[derive(Deserialize)]
pub struct FilePatchRequest {
    pub path: String,
    pub unified_diff: String,
}

#[derive(Deserialize)]
pub struct FileInsertRequest {
    pub path: String,
    pub line: usize,
    pub content: String,
}

#[derive(Deserialize)]
pub struct FileDeleteLinesRequest {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
}

pub async fn post_file_upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut remote_path: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                file_bytes = field.bytes().await.ok().map(|b| b.to_vec());
            }
            "path" => {
                remote_path = field.text().await.ok();
            }
            _ => {}
        }
    }

    let bytes = match file_bytes {
        Some(b) => b,
        None => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "no file", "code": "NO_FILE" }))).into_response(),
    };

    let path = match remote_path {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "no path", "code": "NO_PATH" }))).into_response(),
    };

    // Create parent directories
    if let Some(parent) = Path::new(&path).parent() {
        let _ = fs::create_dir_all(parent).await;
    }

    let size = bytes.len() as u64;
    match fs::write(&path, &bytes).await {
        Ok(_) => {
            state.metrics.add_bytes_uploaded(size);
            (StatusCode::OK, Json(json!({ "ok": true, "path": path, "size": size }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string(), "code": "WRITE_ERROR" }))).into_response(),
    }
}

pub async fn get_file_download(
    State(state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> impl IntoResponse {
    match fs::read(&q.path).await {
        Ok(bytes) => {
            let size = bytes.len() as u64;
            state.metrics.add_bytes_downloaded(size);
            let filename = Path::new(&q.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file");
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(Body::from(bytes))
                .unwrap()
        }
        Err(e) => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from(json!({ "error": e.to_string(), "code": "NOT_FOUND" }).to_string()))
            .unwrap(),
    }
}

pub async fn post_file_write(
    State(_state): State<AppState>,
    Json(req): Json<FileWriteRequest>,
) -> impl IntoResponse {
    if let Some(parent) = Path::new(&req.path).parent() {
        let _ = fs::create_dir_all(parent).await;
    }

    match fs::write(&req.path, req.content.as_bytes()).await {
        Ok(_) => {
            // Apply mode if specified
            #[cfg(unix)]
            if let Some(mode_str) = &req.mode {
                if let Ok(mode) = u32::from_str_radix(mode_str, 8) {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(mode);
                    let _ = std::fs::set_permissions(&req.path, perms);
                }
            }
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string(), "code": "WRITE_ERROR" }))).into_response(),
    }
}

pub async fn get_file_read(
    State(_state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> impl IntoResponse {
    match fs::read(&q.path).await {
        Ok(bytes) => {
            let size = bytes.len() as u64;
            let content = String::from_utf8_lossy(&bytes).into_owned();
            (StatusCode::OK, Json(json!({ "content": content, "size": size }))).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string(), "code": "NOT_FOUND" }))).into_response(),
    }
}

pub async fn post_file_str_replace(
    State(_state): State<AppState>,
    Json(req): Json<FileStrReplaceRequest>,
) -> impl IntoResponse {
    let content = match fs::read_to_string(&req.path).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string(), "code": "NOT_FOUND" }))).into_response(),
    };

    if !content.contains(&req.old_str) {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "error": "old_str not found", "code": "NOT_FOUND_IN_FILE" }))).into_response();
    }

    let new_content = content.replacen(&req.old_str, &req.new_str, 1);
    match fs::write(&req.path, new_content.as_bytes()).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string(), "code": "WRITE_ERROR" }))).into_response(),
    }
}

pub async fn post_file_patch(
    State(_state): State<AppState>,
    Json(req): Json<FilePatchRequest>,
) -> impl IntoResponse {
    // Write diff to temp file and apply with patch command
    let diff_path = format!("/tmp/patch_{}.diff", uuid::Uuid::new_v4());
    if let Err(e) = fs::write(&diff_path, req.unified_diff.as_bytes()).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string(), "code": "TEMP_WRITE_ERROR" }))).into_response();
    }

    let output = tokio::process::Command::new("patch")
        .args([&req.path, &diff_path])
        .output()
        .await;

    let _ = fs::remove_file(&diff_path).await;

    match output {
        Ok(o) if o.status.success() => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Ok(o) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "error": String::from_utf8_lossy(&o.stderr).to_string(), "code": "PATCH_FAILED" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string(), "code": "PATCH_ERROR" }))).into_response(),
    }
}

pub async fn post_file_insert(
    State(_state): State<AppState>,
    Json(req): Json<FileInsertRequest>,
) -> impl IntoResponse {
    let content = match fs::read_to_string(&req.path).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string(), "code": "NOT_FOUND" }))).into_response(),
    };

    let mut lines: Vec<&str> = content.lines().collect();
    let insert_at = req.line.min(lines.len());
    let insert_content = req.content.clone();

    // We need to own lines
    let mut owned_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    owned_lines.insert(insert_at, insert_content);
    let new_content = owned_lines.join("\n");

    match fs::write(&req.path, new_content.as_bytes()).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string(), "code": "WRITE_ERROR" }))).into_response(),
    }
}

pub async fn post_file_delete_lines(
    State(_state): State<AppState>,
    Json(req): Json<FileDeleteLinesRequest>,
) -> impl IntoResponse {
    let content = match fs::read_to_string(&req.path).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string(), "code": "NOT_FOUND" }))).into_response(),
    };

    let lines: Vec<&str> = content.lines().collect();
    let start = req.start_line.saturating_sub(1); // 1-indexed to 0-indexed
    let end = req.end_line.min(lines.len());

    let new_lines: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i < start || *i >= end)
        .map(|(_, l)| *l)
        .collect();

    let new_content = new_lines.join("\n");
    match fs::write(&req.path, new_content.as_bytes()).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string(), "code": "WRITE_ERROR" }))).into_response(),
    }
}
