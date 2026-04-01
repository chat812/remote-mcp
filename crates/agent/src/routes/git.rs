use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Command;

use super::{error_response, AppState};

#[derive(Debug, Deserialize)]
pub struct RepoQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    pub path: String,
    pub n: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct DiffQuery {
    pub path: String,
    pub staged: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct PullBody {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct CheckoutBody {
    pub path: String,
    pub branch_or_commit: String,
}

fn run_git(args: &[&str], workdir: &str) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;

    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

pub async fn get_git_status(
    State(_state): State<AppState>,
    Query(q): Query<RepoQuery>,
) -> impl IntoResponse {
    let status = match run_git(&["status", "--porcelain=v2", "--branch"], &q.path) {
        Ok(s) => s,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "GIT_ERROR", &e).into_response(),
    };

    // Parse porcelain v2 output
    let mut branch = String::new();
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let mut staged = 0u32;
    let mut unstaged = 0u32;
    let mut untracked = 0u32;

    for line in status.lines() {
        if line.starts_with("# branch.head ") {
            branch = line.trim_start_matches("# branch.head ").to_string();
        } else if line.starts_with("# branch.ab ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                ahead = parts[2].trim_start_matches('+').parse().unwrap_or(0);
                behind = parts[3].trim_start_matches('-').parse().unwrap_or(0);
            }
        } else if line.starts_with('1') || line.starts_with('2') {
            // Changed tracked files
            staged += 1;
        } else if line.starts_with('u') {
            // Unmerged
            staged += 1;
        } else if line.starts_with('?') {
            untracked += 1;
        }
    }

    Json(json!({
        "branch": branch,
        "ahead": ahead,
        "behind": behind,
        "staged": staged,
        "unstaged": unstaged,
        "untracked": untracked,
    }))
    .into_response()
}

pub async fn get_git_log(
    State(_state): State<AppState>,
    Query(q): Query<LogQuery>,
) -> impl IntoResponse {
    let n = q.n.unwrap_or(20);
    let n_str = n.to_string();
    let format = "%H%x1f%an%x1f%ad%x1f%s%x1e";

    let out = match run_git(
        &["log", "--format", format, "--date=iso-strict", &format!("-{n_str}")],
        &q.path,
    ) {
        Ok(s) => s,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "GIT_ERROR", &e).into_response(),
    };

    let entries: Vec<serde_json::Value> = out
        .split('\x1e')
        .filter(|s| !s.trim().is_empty())
        .map(|entry| {
            let parts: Vec<&str> = entry.trim().splitn(4, '\x1f').collect();
            json!({
                "hash": parts.first().unwrap_or(&""),
                "author": parts.get(1).unwrap_or(&""),
                "date": parts.get(2).unwrap_or(&""),
                "message": parts.get(3).unwrap_or(&""),
            })
        })
        .collect();

    Json(json!({ "entries": entries })).into_response()
}

pub async fn get_git_diff(
    State(_state): State<AppState>,
    Query(q): Query<DiffQuery>,
) -> impl IntoResponse {
    let args = if q.staged.unwrap_or(false) {
        vec!["diff", "--cached"]
    } else {
        vec!["diff"]
    };

    let out = match run_git(&args, &q.path) {
        Ok(s) => s,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "GIT_ERROR", &e).into_response(),
    };

    // Paginate at 100KB
    const MAX: usize = 100 * 1024;
    let (diff, truncated) = if out.len() > MAX {
        (&out[..MAX], true)
    } else {
        (out.as_str(), false)
    };

    Json(json!({ "diff": diff, "truncated": truncated })).into_response()
}

pub async fn post_git_pull(
    State(_state): State<AppState>,
    Json(body): Json<PullBody>,
) -> impl IntoResponse {
    match run_git(&["pull"], &body.path) {
        Ok(out) => Json(json!({ "output": out })).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "GIT_ERROR", &e).into_response(),
    }
}

pub async fn post_git_checkout(
    State(_state): State<AppState>,
    Json(body): Json<CheckoutBody>,
) -> impl IntoResponse {
    match run_git(&["checkout", &body.branch_or_commit], &body.path) {
        Ok(out) => Json(json!({ "output": out })).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "GIT_ERROR", &e).into_response(),
    }
}
