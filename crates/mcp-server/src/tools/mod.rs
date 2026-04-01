pub mod docker;
pub mod env;
pub mod exec;
pub mod file;
pub mod fleet;
pub mod fs;
pub mod git;
pub mod logs;
pub mod machine;
pub mod process;
pub mod service;
pub mod session;
pub mod sysinfo;
pub mod ui;

use crate::audit::{redact_args, AuditEntry, AuditLog};
use crate::db::{Db, Machine};
use crate::error::RemoteExecError;
use crate::transport::CircuitBreakers;
use anyhow::Result;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tracing::info;

pub const OUTPUT_LIMIT: usize = 100 * 1024; // 100 KB

pub fn paginate(output: String, _total_hint: Option<usize>) -> String {
    if output.len() <= OUTPUT_LIMIT {
        return output;
    }
    let truncated = &output[..OUTPUT_LIMIT];
    format!(
        "{}\n\n[Output truncated at 100KB. Use pagination to see more.]",
        truncated
    )
}

pub fn compute_enabled_tools(_machines: &[Machine]) -> HashSet<&'static str> {
    // All tools are always exposed regardless of registered machines or their
    // capabilities.  Gating tools on runtime state causes Claude Code to miss
    // tools that become available after session start (tools/list_changed is
    // not picked up mid-session).  Tools return a clear error when invoked
    // against a machine that lacks the required capability.
    HashSet::from([
        // Machine management
        "machine_add", "machine_list", "machine_remove", "machine_test",
        // Execution
        "exec", "job_start", "job_status", "job_logs", "job_kill", "job_list",
        // File operations
        "file_upload", "file_download", "file_write", "file_read",
        "file_str_replace", "file_patch", "file_insert", "file_delete_lines",
        // Filesystem
        "fs_ls", "fs_stat", "fs_find", "fs_tree", "fs_mkdir", "fs_rm", "fs_mv", "fs_cp",
        // Processes
        "ps_list", "ps_kill", "ps_tree",
        // Logs / sysinfo / network
        "log_tail", "log_grep",
        "sys_info", "disk_usage",
        "net_ports", "net_ping", "net_interfaces",
        // Environment
        "env_set", "env_get", "env_unset", "env_load", "env_clear",
        // Fleet
        "fleet_exec", "fleet_ls", "fleet_upload",
        // Sessions (PTY)
        "session_open", "session_exec", "session_close", "session_list",
        // Systemd services
        "service_list", "service_status", "service_start", "service_stop",
        "service_restart", "service_enable", "service_disable", "service_logs",
        // Docker
        "docker_ps", "docker_logs", "docker_exec", "docker_start", "docker_stop",
        "docker_restart", "docker_inspect", "docker_images",
        // Git
        "git_status", "git_log", "git_diff", "git_pull", "git_checkout",
        // Windows UI Automation
        "ui_describe", "ui_ocr",
        "ui_windows", "ui_tree", "ui_focus", "ui_click", "ui_move",
        "ui_type", "ui_key", "ui_scroll",
        "ui_find_element", "ui_click_element", "ui_get_value", "ui_set_value",
        "ui_screenshot",
    ])
}

#[derive(Clone)]
pub struct ToolContext {
    pub db: Db,
    pub audit: AuditLog,
    pub circuits: Arc<CircuitBreakers>,
}

impl ToolContext {
    pub fn new(db: Db, audit: AuditLog, circuits: Arc<CircuitBreakers>) -> Self {
        Self { db, audit, circuits }
    }

    pub async fn invoke<F, Fut>(
        &self,
        tool: &str,
        machine_id: Option<&str>,
        args: serde_json::Value,
        f: F,
    ) -> String
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<String>>,
    {
        let start = Instant::now();
        let (machine_id_str, label) = if let Some(mid) = machine_id {
            match self.db.get(mid) {
                Ok(Some(m)) => (m.id.clone(), m.label.clone()),
                _ => (mid.to_string(), mid.to_string()),
            }
        } else {
            ("".to_string(), "".to_string())
        };

        let result = f().await;
        let duration_ms = start.elapsed().as_millis() as u64;
        let ok = result.is_ok();

        let entry = AuditEntry {
            ts: chrono::Utc::now().timestamp(),
            tool: tool.to_string(),
            machine_id: machine_id_str,
            label,
            args: redact_args(&args),
            ok,
            duration_ms,
            exit_code: None,
        };
        self.audit.record(entry);

        match result {
            Ok(s) => s,
            Err(e) => format!("Error: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Capabilities, Machine};

    fn make_machine_with_caps(caps: Capabilities) -> Machine {
        Machine {
            id: uuid::Uuid::new_v4().to_string(),
            label: "test".into(),
            host: "127.0.0.1".into(),
            port: 22,
            os: "linux".into(),
            transport: "agent".into(),
            ssh_user: None,
            ssh_key_path: None,
            ssh_password: None,
            agent_url: Some("http://localhost:8765".into()),
            agent_token: Some("tok".into()),
            capabilities: Some(caps),
            last_seen: None,
            status: "online".into(),
            created_at: 0,
        }
    }

    // --- paginate ---

    #[test]
    fn paginate_short_output_unchanged() {
        let s = "hello world".to_string();
        assert_eq!(paginate(s.clone(), None), s);
    }

    #[test]
    fn paginate_exactly_at_limit_unchanged() {
        let s = "x".repeat(OUTPUT_LIMIT);
        let result = paginate(s.clone(), None);
        assert_eq!(result, s);
    }

    #[test]
    fn paginate_over_limit_is_truncated() {
        let s = "x".repeat(OUTPUT_LIMIT + 1000);
        let result = paginate(s, None);
        assert!(result.len() <= OUTPUT_LIMIT + 200);
        assert!(result.contains("[Output truncated"));
    }

    #[test]
    fn paginate_empty_string() {
        assert_eq!(paginate(String::new(), None), "");
    }

    // --- compute_enabled_tools ---
    // All tools are always present regardless of machines or capabilities.

    #[test]
    fn all_tools_present_with_no_machines() {
        let tools = compute_enabled_tools(&[]);
        // machine management
        assert!(tools.contains("machine_add"));
        assert!(tools.contains("machine_list"));
        // exec / file / fs
        assert!(tools.contains("exec"));
        assert!(tools.contains("file_read"));
        assert!(tools.contains("fs_ls"));
        assert!(tools.contains("session_open"));
        // capability-gated tools also always present
        assert!(tools.contains("docker_ps"));
        assert!(tools.contains("git_status"));
        assert!(tools.contains("service_list"));
    }

    #[test]
    fn all_tools_present_regardless_of_caps() {
        let caps = Capabilities { has_docker: false, has_git: false, has_systemd: false, ..Default::default() };
        let m = make_machine_with_caps(caps);
        let tools = compute_enabled_tools(&[m]);
        assert!(tools.contains("docker_ps"));
        assert!(tools.contains("git_status"));
        assert!(tools.contains("service_list"));
        assert!(tools.contains("exec"));
    }
}
