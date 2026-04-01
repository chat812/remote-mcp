use crate::routes::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use tokio::fs;

#[derive(Deserialize)]
pub struct PathQuery {
    pub path: String,
    pub all: Option<bool>,
    pub depth: Option<i32>,
}

#[derive(Deserialize)]
pub struct FindRequest {
    pub path: String,
    pub pattern: Option<String>,
    pub file_type: Option<String>,
    pub max_depth: Option<i32>,
}

#[derive(Deserialize)]
pub struct MkdirRequest {
    pub path: String,
    pub parents: Option<bool>,
}

#[derive(Deserialize)]
pub struct RmQuery {
    pub path: String,
    pub recursive: Option<bool>,
}

#[derive(Deserialize)]
pub struct MvRequest {
    pub src: String,
    pub dst: String,
}

#[derive(Deserialize)]
pub struct CpRequest {
    pub src: String,
    pub dst: String,
    pub recursive: Option<bool>,
}

pub async fn get_ls(
    State(_state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> impl IntoResponse {
    let path = Path::new(&q.path);

    match fs::read_dir(path).await {
        Ok(mut dir) => {
            let mut entries = Vec::new();
            while let Ok(Some(entry)) = dir.next_entry().await {
                if let Ok(meta) = entry.metadata().await {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !q.all.unwrap_or(false) && name.starts_with('.') {
                        continue;
                    }
                    entries.push(json!({
                        "name": name,
                        "path": entry.path().to_string_lossy(),
                        "is_dir": meta.is_dir(),
                        "is_file": meta.is_file(),
                        "is_symlink": meta.is_symlink(),
                        "size": meta.len(),
                        "modified": meta.modified().ok().and_then(|t| {
                            t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
                        }),
                    }));
                }
            }
            (StatusCode::OK, Json(json!({ "entries": entries }))).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string(), "code": "NOT_FOUND" })),
        )
            .into_response(),
    }
}

pub async fn get_stat(
    State(_state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> impl IntoResponse {
    match fs::metadata(&q.path).await {
        Ok(meta) => (
            StatusCode::OK,
            Json(json!({
                "path": q.path,
                "is_dir": meta.is_dir(),
                "is_file": meta.is_file(),
                "is_symlink": meta.is_symlink(),
                "size": meta.len(),
                "modified": meta.modified().ok().and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
                }),
                "created": meta.created().ok().and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
                }),
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string(), "code": "NOT_FOUND" })),
        )
            .into_response(),
    }
}

pub async fn post_find(
    State(_state): State<AppState>,
    Json(req): Json<FindRequest>,
) -> impl IntoResponse {
    let mut results = Vec::new();
    find_recursive(
        req.path.clone(),
        req.pattern.clone(),
        req.file_type.clone(),
        req.max_depth.unwrap_or(10),
        0,
        &mut results,
    )
    .await;

    (StatusCode::OK, Json(json!({ "results": results }))).into_response()
}

fn find_recursive(
    path: String,
    pattern: Option<String>,
    file_type: Option<String>,
    max_depth: i32,
    current_depth: i32,
    results: &mut Vec<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
    Box::pin(async move {
        if current_depth >= max_depth {
            return;
        }

        let Ok(mut dir) = fs::read_dir(&path).await else { return };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let entry_path = entry.path();
            let entry_path_str = entry_path.to_string_lossy().into_owned();
            let name = entry.file_name().to_string_lossy().into_owned();

            let Ok(meta) = entry.metadata().await else { continue };

            let matches_type = match file_type.as_deref() {
                Some("file") | Some("f") => meta.is_file(),
                Some("dir") | Some("d") => meta.is_dir(),
                Some("symlink") | Some("l") => meta.is_symlink(),
                _ => true,
            };

            let matches_pattern = match pattern.as_deref() {
                Some(p) => name.contains(p) || glob_match(p, &name),
                None => true,
            };

            if matches_type && matches_pattern {
                results.push(entry_path_str.clone());
            }

            if meta.is_dir() {
                find_recursive(
                    entry_path_str,
                    pattern.clone(),
                    file_type.clone(),
                    max_depth,
                    current_depth + 1,
                    results,
                )
                .await;
            }
        }
    })
}

fn glob_match(pattern: &str, name: &str) -> bool {
    // Simple glob: only * wildcard
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').collect();
        let mut pos = 0;
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            if i == 0 {
                if !name.starts_with(part) {
                    return false;
                }
                pos = part.len();
            } else if i == parts.len() - 1 {
                if !name.ends_with(part) {
                    return false;
                }
            } else {
                if let Some(idx) = name[pos..].find(part) {
                    pos += idx + part.len();
                } else {
                    return false;
                }
            }
        }
        true
    } else {
        name == pattern
    }
}

pub async fn get_tree(
    State(_state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> impl IntoResponse {
    let max_depth = q.depth.unwrap_or(3);
    let mut tree = Vec::new();
    build_tree(q.path.clone(), max_depth, 0, &mut tree).await;
    (StatusCode::OK, Json(json!({ "tree": tree }))).into_response()
}

fn build_tree(
    path: String,
    max_depth: i32,
    current_depth: i32,
    tree: &mut Vec<serde_json::Value>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
    Box::pin(async move {
        if current_depth >= max_depth {
            return;
        }

        let Ok(mut dir) = fs::read_dir(&path).await else { return };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let entry_path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata().await else { continue };

            let mut node = json!({
                "name": name,
                "path": entry_path.to_string_lossy(),
                "is_dir": meta.is_dir(),
            });

            if meta.is_dir() {
                let mut children = Vec::new();
                build_tree(
                    entry_path.to_string_lossy().into_owned(),
                    max_depth,
                    current_depth + 1,
                    &mut children,
                )
                .await;
                node["children"] = json!(children);
            } else {
                node["size"] = json!(meta.len());
            }

            tree.push(node);
        }
    })
}

pub async fn post_mkdir(
    State(_state): State<AppState>,
    Json(req): Json<MkdirRequest>,
) -> impl IntoResponse {
    let result = if req.parents.unwrap_or(false) {
        fs::create_dir_all(&req.path).await
    } else {
        fs::create_dir(&req.path).await
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string(), "code": "MKDIR_ERROR" })),
        )
            .into_response(),
    }
}

pub async fn delete_rm(
    State(_state): State<AppState>,
    Query(q): Query<RmQuery>,
) -> impl IntoResponse {
    let result = if q.recursive.unwrap_or(false) {
        fs::remove_dir_all(&q.path).await
    } else {
        // Try file first, then dir
        match fs::remove_file(&q.path).await {
            Ok(_) => return (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
            Err(_) => fs::remove_dir(&q.path).await,
        }
    };

    match result {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string(), "code": "RM_ERROR" })),
        )
            .into_response(),
    }
}

pub async fn post_mv(
    State(_state): State<AppState>,
    Json(req): Json<MvRequest>,
) -> impl IntoResponse {
    match fs::rename(&req.src, &req.dst).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string(), "code": "MV_ERROR" })),
        )
            .into_response(),
    }
}

pub async fn post_cp(
    State(_state): State<AppState>,
    Json(req): Json<CpRequest>,
) -> impl IntoResponse {
    if req.recursive.unwrap_or(false) {
        // Recursive copy
        if let Err(e) = copy_dir_recursive(&req.src, &req.dst).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string(), "code": "CP_ERROR" })),
            )
                .into_response();
        }
    } else {
        if let Err(e) = fs::copy(&req.src, &req.dst).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string(), "code": "CP_ERROR" })),
            )
                .into_response();
        }
    }
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

async fn copy_dir_recursive(src: &str, dst: &str) -> std::io::Result<()> {
    fs::create_dir_all(dst).await?;
    let mut dir = fs::read_dir(src).await?;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let src_path = entry.path();
        let dst_path = Path::new(dst).join(entry.file_name());
        let meta = entry.metadata().await?;
        if meta.is_dir() {
            Box::pin(copy_dir_recursive(
                &src_path.to_string_lossy(),
                &dst_path.to_string_lossy(),
            ))
            .await?;
        } else {
            fs::copy(&src_path, &dst_path).await?;
        }
    }
    Ok(())
}
