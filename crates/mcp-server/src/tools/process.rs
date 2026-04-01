use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{agent, dispatch_exec, CircuitBreakers};
use anyhow::Result;
use std::sync::Arc;

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub async fn ps_list(
    db: &Db,
    machine_id: &str,
    filter: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = if let Some(f) = filter {
            format!("/process/list?filter={}", urlenc(f))
        } else {
            "/process/list".to_string()
        };
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = if let Some(f) = filter {
            format!("ps aux | grep {}", shell_escape(f))
        } else {
            "ps aux".to_string()
        };
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn ps_kill(
    db: &Db,
    machine_id: &str,
    pid: u32,
    signal: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "pid": pid, "signal": signal.unwrap_or("TERM") });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/process/kill", &req, 10).await?;
        Ok(format!("Signal sent to PID {}", pid))
    } else {
        let sig = signal.unwrap_or("TERM");
        let cmd = format!("kill -{} {}", sig, pid);
        let r = dispatch_exec(&machine, &cmd, 10, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("kill failed: {}", r.stderr));
        }
        Ok(format!("Signal {} sent to PID {}", sig, pid))
    }
}

pub async fn ps_tree(
    db: &Db,
    machine_id: &str,
    pid: Option<u32>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = if let Some(p) = pid {
            format!("/process/tree?pid={}", p)
        } else {
            "/process/tree".to_string()
        };
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = if let Some(p) = pid {
            format!("pstree -p {}", p)
        } else {
            "pstree -p".to_string()
        };
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}
