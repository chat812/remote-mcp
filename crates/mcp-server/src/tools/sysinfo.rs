use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{agent, dispatch_exec, CircuitBreakers};
use anyhow::Result;
use std::sync::Arc;

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub async fn sys_info(
    db: &Db,
    machine_id: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let result: serde_json::Value = agent::agent_get_json(&machine, "/sysinfo", 30).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()))
    } else {
        let cmd = "uname -a; uptime; free -h; nproc";
        let r = dispatch_exec(&machine, cmd, 30, &circuits).await?;
        Ok(r.stdout)
    }
}

pub async fn disk_usage(
    db: &Db,
    machine_id: &str,
    path: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = if let Some(p) = path {
            format!("/sysinfo/disk?path={}", urlenc(p))
        } else {
            "/sysinfo/disk".to_string()
        };
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = if let Some(p) = path {
            format!("df -h {} && du -sh {}/*", shell_escape(p), shell_escape(p))
        } else {
            "df -h".to_string()
        };
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(r.stdout)
    }
}

pub async fn net_ports(
    db: &Db,
    machine_id: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let result: serde_json::Value = agent::agent_get_json(&machine, "/sysinfo/ports", 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = "ss -tlnp 2>/dev/null || netstat -tlnp 2>/dev/null";
        let r = dispatch_exec(&machine, cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn net_ping(
    db: &Db,
    machine_id: &str,
    target: &str,
    count: Option<u32>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "target": target, "count": count.unwrap_or(4) });
        let result: serde_json::Value = agent::agent_post_json(&machine, "/sysinfo/ping", &req, 30).await?;
        Ok(result.to_string())
    } else {
        let n = count.unwrap_or(4);
        let cmd = format!("ping -c {} {}", n, shell_escape(target));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(r.stdout + &r.stderr)
    }
}

pub async fn net_interfaces(
    db: &Db,
    machine_id: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let result: serde_json::Value = agent::agent_get_json(&machine, "/sysinfo/interfaces", 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = "ip addr show 2>/dev/null || ifconfig 2>/dev/null";
        let r = dispatch_exec(&machine, cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
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
