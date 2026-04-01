use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{agent, dispatch_exec, CircuitBreakers};
use anyhow::Result;
use std::sync::Arc;

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
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

pub async fn log_tail(
    db: &Db,
    machine_id: &str,
    path: &str,
    tail: Option<usize>,
    cursor: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let mut url = format!("/log/tail?path={}", urlenc(path));
        if let Some(t) = tail {
            url.push_str(&format!("&tail={}", t));
        }
        if let Some(c) = cursor {
            url.push_str(&format!("&cursor={}", urlenc(c)));
        }
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let n = tail.unwrap_or(100);
        let cmd = format!("tail -n {} {}", n, shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn log_grep(
    db: &Db,
    machine_id: &str,
    path: &str,
    pattern: &str,
    context: Option<usize>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = format!("/log/grep?path={}&pattern={}&context={}",
            urlenc(path), urlenc(pattern), context.unwrap_or(0));
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let ctx_arg = context.map(|c| format!("-C {}", c)).unwrap_or_default();
        let cmd = format!("grep -n {} {} {}", ctx_arg, shell_escape(pattern), shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}
