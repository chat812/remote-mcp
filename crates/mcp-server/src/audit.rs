use anyhow::Result;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const MAX_ROTATIONS: u32 = 5;

#[derive(Serialize)]
pub struct AuditEntry {
    pub ts: i64,
    pub tool: String,
    pub machine_id: String,
    pub label: String,
    pub args: serde_json::Value,
    pub ok: bool,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
}

fn audit_log_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("remote-exec-mcp")
        .join("audit.log")
}

#[derive(Clone)]
pub struct AuditLog {
    inner: Arc<Mutex<AuditLogInner>>,
}

struct AuditLogInner {
    path: PathBuf,
    file: File,
}

impl AuditLog {
    pub fn open() -> Result<Self> {
        let path = audit_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(AuditLogInner { path, file })),
        })
    }

    pub fn record(&self, entry: AuditEntry) {
        let line = match serde_json::to_string(&entry) {
            Ok(l) => l,
            Err(_) => return,
        };
        let mut inner = self.inner.lock().unwrap();
        let _ = writeln!(inner.file, "{}", line);
        let _ = inner.file.flush();
        // Check rotation
        if let Ok(meta) = inner.path.metadata() {
            if meta.len() > MAX_LOG_SIZE {
                let _ = rotate_logs(&inner.path);
                if let Ok(f) = OpenOptions::new().create(true).append(true).open(&inner.path) {
                    inner.file = f;
                }
            }
        }
    }
}

fn rotate_logs(path: &PathBuf) -> Result<()> {
    // Remove oldest
    let oldest = path.with_extension(format!("log.{}", MAX_ROTATIONS));
    if oldest.exists() {
        fs::remove_file(&oldest)?;
    }
    // Rotate existing
    for i in (1..MAX_ROTATIONS).rev() {
        let from = path.with_extension(format!("log.{}", i));
        let to = path.with_extension(format!("log.{}", i + 1));
        if from.exists() {
            fs::rename(&from, &to)?;
        }
    }
    // Rename current
    let backup = path.with_extension("log.1");
    fs::rename(path, backup)?;
    Ok(())
}

/// Redact sensitive fields from args before logging
pub fn redact_args(args: &serde_json::Value) -> serde_json::Value {

    const REDACTED_KEYS: &[&str] = &[
        "ssh_password",
        "agent_token",
        "content",
        "unified_diff",
        "password",
        "token",
    ];
    match args {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if REDACTED_KEYS.contains(&k.as_str()) {
                    out.insert(k.clone(), serde_json::Value::String("[REDACTED]".to_string()));
                } else {
                    out.insert(k.clone(), redact_args(v));
                }
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redact_ssh_password() {
        let args = json!({ "machine_id": "m1", "ssh_password": "s3cr3t" });
        let out = redact_args(&args);
        assert_eq!(out["ssh_password"], "[REDACTED]");
        assert_eq!(out["machine_id"], "m1");
    }

    #[test]
    fn redact_agent_token() {
        let args = json!({ "agent_token": "tok123", "label": "prod" });
        let out = redact_args(&args);
        assert_eq!(out["agent_token"], "[REDACTED]");
        assert_eq!(out["label"], "prod");
    }

    #[test]
    fn redact_content_field() {
        let args = json!({ "path": "/etc/file", "content": "secret file contents" });
        let out = redact_args(&args);
        assert_eq!(out["content"], "[REDACTED]");
        assert_eq!(out["path"], "/etc/file");
    }

    #[test]
    fn redact_unified_diff() {
        let args = json!({ "path": "/app/main.rs", "unified_diff": "--- a\n+++ b\n@@ ..." });
        let out = redact_args(&args);
        assert_eq!(out["unified_diff"], "[REDACTED]");
    }

    #[test]
    fn redact_password_and_token() {
        let args = json!({ "password": "hunter2", "token": "abc" });
        let out = redact_args(&args);
        assert_eq!(out["password"], "[REDACTED]");
        assert_eq!(out["token"], "[REDACTED]");
    }

    #[test]
    fn preserves_safe_fields() {
        let args = json!({
            "machine_id": "m1",
            "command": "ls -la",
            "timeout": 30
        });
        let out = redact_args(&args);
        assert_eq!(out["machine_id"], "m1");
        assert_eq!(out["command"], "ls -la");
        assert_eq!(out["timeout"], 30);
    }

    #[test]
    fn redact_nested_object() {
        let args = json!({
            "outer": "ok",
            "nested": {
                "ssh_password": "deep_secret",
                "safe": "value"
            }
        });
        let out = redact_args(&args);
        assert_eq!(out["nested"]["ssh_password"], "[REDACTED]");
        assert_eq!(out["nested"]["safe"], "value");
        assert_eq!(out["outer"], "ok");
    }

    #[test]
    fn non_object_passthrough() {
        assert_eq!(redact_args(&json!("string")), json!("string"));
        assert_eq!(redact_args(&json!(42)), json!(42));
        assert_eq!(redact_args(&json!(true)), json!(true));
        assert_eq!(redact_args(&json!(null)), json!(null));
    }

    #[test]
    fn empty_object() {
        let out = redact_args(&json!({}));
        assert_eq!(out, json!({}));
    }

    #[test]
    fn redact_all_sensitive_at_once() {
        let args = json!({
            "ssh_password": "p1",
            "agent_token": "t1",
            "content": "c1",
            "unified_diff": "d1",
            "password": "p2",
            "token": "t2",
            "machine_id": "safe"
        });
        let out = redact_args(&args);
        for key in &["ssh_password", "agent_token", "content", "unified_diff", "password", "token"] {
            assert_eq!(out[key], "[REDACTED]", "key {} should be redacted", key);
        }
        assert_eq!(out["machine_id"], "safe");
    }
}
