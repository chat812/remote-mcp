use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{agent, dispatch_exec, CircuitBreakers};
use anyhow::Result;
use std::sync::Arc;

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub async fn docker_ps(
    db: &Db,
    machine_id: &str,
    all: bool,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = format!("/docker/ps?all={}", all);
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let flags = if all { "-a" } else { "" };
        let cmd = format!("docker ps {} --format 'table {{{{.ID}}}}\\t{{{{.Image}}}}\\t{{{{.Status}}}}\\t{{{{.Names}}}}'", flags);
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(r.stdout)
    }
}

pub async fn docker_logs(
    db: &Db,
    machine_id: &str,
    container: &str,
    tail: Option<usize>,
    follow: bool,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = format!("/docker/{}/logs?tail={}", container, tail.unwrap_or(100));
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let n = tail.unwrap_or(100);
        let cmd = format!("docker logs --tail {} {}", n, shell_escape(container));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout + &r.stderr, None))
    }
}

pub async fn docker_exec(
    db: &Db,
    machine_id: &str,
    container: &str,
    command: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "command": command });
        let result: serde_json::Value = agent::agent_post_json(&machine, &format!("/docker/{}/exec", container), &req, 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = format!("docker exec {} sh -c {}", shell_escape(container), shell_escape(command));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout + &r.stderr, None))
    }
}

async fn docker_action(
    db: &Db,
    machine_id: &str,
    container: &str,
    action: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let _: serde_json::Value = agent::agent_post_json(
            &machine,
            &format!("/docker/{}/{}", container, action),
            &serde_json::json!({}),
            30,
        ).await?;
        Ok(format!("Container {} {}ed", container, action))
    } else {
        let cmd = format!("docker {} {}", action, shell_escape(container));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("docker {} failed: {}", action, r.stderr));
        }
        Ok(format!("Container {} {}ed", container, action))
    }
}

pub async fn docker_start(db: &Db, machine_id: &str, container: &str, circuits: Arc<CircuitBreakers>) -> Result<String> {
    docker_action(db, machine_id, container, "start", circuits).await
}

pub async fn docker_stop(db: &Db, machine_id: &str, container: &str, circuits: Arc<CircuitBreakers>) -> Result<String> {
    docker_action(db, machine_id, container, "stop", circuits).await
}

pub async fn docker_restart(db: &Db, machine_id: &str, container: &str, circuits: Arc<CircuitBreakers>) -> Result<String> {
    docker_action(db, machine_id, container, "restart", circuits).await
}

pub async fn docker_inspect(
    db: &Db,
    machine_id: &str,
    container: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let result: serde_json::Value = agent::agent_get_json(&machine, &format!("/docker/{}/inspect", container), 30).await?;
        Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()))
    } else {
        let cmd = format!("docker inspect {}", shell_escape(container));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn docker_images(
    db: &Db,
    machine_id: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let result: serde_json::Value = agent::agent_get_json(&machine, "/docker/images", 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = "docker images --format 'table {{.ID}}\\t{{.Repository}}\\t{{.Tag}}\\t{{.Size}}'";
        let r = dispatch_exec(&machine, cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}
