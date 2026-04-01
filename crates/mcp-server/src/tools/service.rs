use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{agent, dispatch_exec, CircuitBreakers};
use anyhow::Result;
use std::sync::Arc;

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

async fn systemctl_cmd(
    db: &Db,
    machine_id: &str,
    action: &str,
    service: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let _: serde_json::Value = agent::agent_post_json(
            &machine,
            &format!("/service/{}/{}", service, action),
            &serde_json::json!({}),
            30,
        ).await?;
        Ok(format!("Service {} {}ed", service, action))
    } else {
        let cmd = format!("systemctl {} {}", action, shell_escape(service));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("systemctl {} failed: {}", action, r.stderr));
        }
        Ok(format!("Service {} {}ed", service, action))
    }
}

pub async fn service_list(
    db: &Db,
    machine_id: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let result: serde_json::Value = agent::agent_get_json(&machine, "/service/list", 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = "systemctl list-units --type=service --no-pager --plain";
        let r = dispatch_exec(&machine, cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn service_status(
    db: &Db,
    machine_id: &str,
    service: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let result: serde_json::Value = agent::agent_get_json(&machine, &format!("/service/{}/status", service), 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = format!("systemctl status {} --no-pager", shell_escape(service));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(r.stdout + &r.stderr)
    }
}

pub async fn service_start(db: &Db, machine_id: &str, service: &str, circuits: Arc<CircuitBreakers>) -> Result<String> {
    systemctl_cmd(db, machine_id, "start", service, circuits).await
}

pub async fn service_stop(db: &Db, machine_id: &str, service: &str, circuits: Arc<CircuitBreakers>) -> Result<String> {
    systemctl_cmd(db, machine_id, "stop", service, circuits).await
}

pub async fn service_restart(db: &Db, machine_id: &str, service: &str, circuits: Arc<CircuitBreakers>) -> Result<String> {
    systemctl_cmd(db, machine_id, "restart", service, circuits).await
}

pub async fn service_enable(db: &Db, machine_id: &str, service: &str, circuits: Arc<CircuitBreakers>) -> Result<String> {
    systemctl_cmd(db, machine_id, "enable", service, circuits).await
}

pub async fn service_disable(db: &Db, machine_id: &str, service: &str, circuits: Arc<CircuitBreakers>) -> Result<String> {
    systemctl_cmd(db, machine_id, "disable", service, circuits).await
}

pub async fn service_logs(
    db: &Db,
    machine_id: &str,
    service: &str,
    tail: Option<usize>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = format!("/service/{}/logs?tail={}", service, tail.unwrap_or(100));
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let n = tail.unwrap_or(100);
        let cmd = format!("journalctl -u {} -n {} --no-pager", shell_escape(service), n);
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}
