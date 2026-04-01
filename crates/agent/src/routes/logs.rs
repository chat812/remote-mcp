use crate::routes::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Deserialize)]
pub struct LogTailQuery {
    pub path: String,
    pub tail: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Deserialize)]
pub struct LogGrepQuery {
    pub path: String,
    pub pattern: String,
    pub context: Option<usize>,
}

pub async fn get_log_tail(
    State(_state): State<AppState>,
    Query(q): Query<LogTailQuery>,
) -> impl IntoResponse {
    let tail = q.tail.unwrap_or(100);

    match fs::File::open(&q.path).await {
        Ok(file) => {
            let reader = BufReader::new(file);
            let mut lines_reader = reader.lines();
            let mut all_lines: Vec<String> = Vec::new();

            while let Ok(Some(line)) = lines_reader.next_line().await {
                all_lines.push(line);
            }

            let start = if all_lines.len() > tail {
                all_lines.len() - tail
            } else {
                0
            };
            let result: Vec<&str> = all_lines[start..].iter().map(|s| s.as_str()).collect();
            let output = result.join("\n");
            let next_cursor = all_lines.len().to_string();

            (StatusCode::OK, Json(json!({
                "lines": result,
                "output": output,
                "total_lines": all_lines.len(),
                "cursor": next_cursor,
            }))).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string(), "code": "NOT_FOUND" })),
        )
            .into_response(),
    }
}

pub async fn get_log_grep(
    State(_state): State<AppState>,
    Query(q): Query<LogGrepQuery>,
) -> impl IntoResponse {
    match fs::read_to_string(&q.path).await {
        Ok(content) => {
            let ctx = q.context.unwrap_or(0);
            let lines: Vec<&str> = content.lines().collect();
            let mut results: Vec<String> = Vec::new();
            let mut included = vec![false; lines.len()];

            // Mark lines matching pattern
            for (i, line) in lines.iter().enumerate() {
                if line.contains(&q.pattern) {
                    let start = i.saturating_sub(ctx);
                    let end = (i + ctx + 1).min(lines.len());
                    for j in start..end {
                        included[j] = true;
                    }
                }
            }

            for (i, line) in lines.iter().enumerate() {
                if included[i] {
                    results.push(format!("{}:{}", i + 1, line));
                }
            }

            (StatusCode::OK, Json(json!({
                "matches": results,
                "count": results.len(),
            }))).into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string(), "code": "NOT_FOUND" })),
        )
            .into_response(),
    }
}
