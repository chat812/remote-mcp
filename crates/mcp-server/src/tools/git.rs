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

pub async fn git_status(
    db: &Db,
    machine_id: &str,
    workdir: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = format!("/git/status?workdir={}", urlenc(workdir));
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = format!("cd {} && git status", shell_escape(workdir));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(r.stdout + &r.stderr)
    }
}

pub async fn git_log(
    db: &Db,
    machine_id: &str,
    workdir: &str,
    n: Option<u32>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = format!("/git/log?workdir={}&n={}", urlenc(workdir), n.unwrap_or(20));
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let count = n.unwrap_or(20);
        let cmd = format!("cd {} && git log --oneline -{}", shell_escape(workdir), count);
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(r.stdout)
    }
}

pub async fn git_diff(
    db: &Db,
    machine_id: &str,
    workdir: &str,
    target: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = if let Some(t) = target {
            format!("/git/diff?workdir={}&target={}", urlenc(workdir), urlenc(t))
        } else {
            format!("/git/diff?workdir={}", urlenc(workdir))
        };
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = if let Some(t) = target {
            format!("cd {} && git diff {}", shell_escape(workdir), shell_escape(t))
        } else {
            format!("cd {} && git diff", shell_escape(workdir))
        };
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn git_pull(
    db: &Db,
    machine_id: &str,
    workdir: &str,
    remote: Option<&str>,
    branch: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({
            "workdir": workdir,
            "remote": remote.unwrap_or("origin"),
            "branch": branch,
        });
        let result: serde_json::Value = agent::agent_post_json(&machine, "/git/pull", &req, 60).await?;
        Ok(result.to_string())
    } else {
        let r_arg = remote.unwrap_or("origin");
        let cmd = if let Some(b) = branch {
            format!("cd {} && git pull {} {}", shell_escape(workdir), r_arg, b)
        } else {
            format!("cd {} && git pull {}", shell_escape(workdir), r_arg)
        };
        let r = dispatch_exec(&machine, &cmd, 60, &circuits).await?;
        Ok(r.stdout + &r.stderr)
    }
}

pub async fn git_checkout(
    db: &Db,
    machine_id: &str,
    workdir: &str,
    branch: &str,
    create: bool,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({
            "workdir": workdir,
            "branch": branch,
            "create": create,
        });
        let result: serde_json::Value = agent::agent_post_json(&machine, "/git/checkout", &req, 30).await?;
        Ok(result.to_string())
    } else {
        let b_flag = if create { "-b" } else { "" };
        let cmd = format!("cd {} && git checkout {} {}", shell_escape(workdir), b_flag, shell_escape(branch));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(r.stdout + &r.stderr)
    }
}
