use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{agent, dispatch_exec, CircuitBreakers};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub async fn env_get(
    db: &Db,
    machine_id: &str,
    key: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = if let Some(k) = key {
            format!("/env/get?key={}", k)
        } else {
            "/env/get".to_string()
        };
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 10).await?;
        Ok(result.to_string())
    } else {
        let cmd = if let Some(k) = key {
            format!("echo ${}", k)
        } else {
            "env".to_string()
        };
        let r = dispatch_exec(&machine, &cmd, 10, &circuits).await?;
        Ok(r.stdout)
    }
}

pub async fn env_set(
    db: &Db,
    machine_id: &str,
    key: &str,
    value: &str,
    scope: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "key": key, "value": value, "scope": scope.unwrap_or("session") });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/env/set", &req, 10).await?;
        Ok(format!("Set {}={}", key, value))
    } else {
        // For persistent: append to /etc/environment
        let profile = scope.unwrap_or("session");
        let cmd = if profile == "global" {
            format!("echo '{}={}' >> /etc/environment", key, value)
        } else {
            format!("echo 'export {}={}' >> ~/.bashrc", key, shell_escape(value))
        };
        let r = dispatch_exec(&machine, &cmd, 10, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("env_set failed: {}", r.stderr));
        }
        Ok(format!("Set {}={} in {}", key, value, profile))
    }
}

pub async fn env_unset(
    db: &Db,
    machine_id: &str,
    key: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "key": key });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/env/unset", &req, 10).await?;
        Ok(format!("Unset {}", key))
    } else {
        let cmd = format!("unset {}", key);
        let r = dispatch_exec(&machine, &cmd, 10, &circuits).await?;
        Ok(format!("Unset {} (exit {})", key, r.exit_code))
    }
}

pub async fn env_load(
    db: &Db,
    machine_id: &str,
    path: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "path": path });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/env/load", &req, 10).await?;
        Ok(format!("Loaded env from {}", path))
    } else {
        let cmd = format!("source {} && env", shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 10, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("env_load failed: {}", r.stderr));
        }
        Ok(format!("Loaded env from {}", path))
    }
}

pub async fn env_clear(
    db: &Db,
    machine_id: &str,
    scope: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "scope": scope.unwrap_or("session") });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/env/clear", &req, 10).await?;
        Ok("Environment cleared".to_string())
    } else {
        Ok("env_clear via SSH is not supported (would clear the current session env)".to_string())
    }
}
