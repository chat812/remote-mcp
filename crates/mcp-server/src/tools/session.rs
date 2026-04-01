use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::agent;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct SessionOpenReq<'a> {
    workdir: Option<&'a str>,
    shell: Option<&'a str>,
}

#[derive(Deserialize)]
struct SessionOpenResp {
    session_id: String,
}

#[derive(Serialize)]
struct SessionExecReq<'a> {
    command: &'a str,
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
struct SessionExecResp {
    stdout: String,
    exit_code: i32,
}

#[derive(Deserialize)]
struct SessionsListResp {
    sessions: Vec<SessionInfo>,
}

#[derive(Deserialize)]
struct SessionInfo {
    id: String,
    cwd: Option<String>,
    idle_secs: Option<u64>,
}

pub async fn session_open(
    db: &Db,
    machine_id: &str,
    workdir: Option<&str>,
    shell: Option<&str>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired { tool: "session_open".to_string() }.into());
    }

    let req = SessionOpenReq { workdir, shell };
    let resp: SessionOpenResp = agent::agent_post_json(&machine, "/session", &req, 10).await?;
    Ok(format!("Session opened: {}", resp.session_id))
}

pub async fn session_exec(
    db: &Db,
    machine_id: &str,
    session_id: &str,
    command: &str,
    timeout_secs: Option<u64>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired { tool: "session_exec".to_string() }.into());
    }

    let req = SessionExecReq { command, timeout_secs };
    let resp: SessionExecResp = agent::agent_post_json(
        &machine,
        &format!("/session/{}/exec", session_id),
        &req,
        timeout_secs.unwrap_or(30) + 5,
    ).await?;

    let mut out = resp.stdout;
    if resp.exit_code != 0 {
        out.push_str(&format!("\n[exit_code: {}]", resp.exit_code));
    }
    Ok(crate::tools::paginate(out, None))
}

pub async fn session_close(
    db: &Db,
    machine_id: &str,
    session_id: &str,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired { tool: "session_close".to_string() }.into());
    }

    let signed = crate::auth::sign_request(
        machine.agent_token.as_deref().unwrap_or(""),
        "DELETE",
        &format!("/session/{}", session_id),
        b"",
    )?;

    let client = reqwest::Client::new();
    let url = format!(
        "{}/session/{}",
        machine.agent_url.as_ref().unwrap().trim_end_matches('/'),
        session_id
    );
    let resp = client
        .delete(&url)
        .header("X-Agent-Timestamp", &signed.timestamp)
        .header("X-Agent-Signature", &signed.signature)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(RemoteExecError::AgentHttpError { machine: machine.id, status, body }.into());
    }

    Ok(format!("Session {} closed.", session_id))
}

pub async fn session_list(
    db: &Db,
    machine_id: &str,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired { tool: "session_list".to_string() }.into());
    }

    let resp: SessionsListResp = agent::agent_get_json(&machine, "/sessions", 10).await?;

    if resp.sessions.is_empty() {
        return Ok("No active sessions.".to_string());
    }

    let mut out = format!("{:<36} {:<30} {:<10}\n", "Session ID", "CWD", "Idle(s)");
    out.push_str(&"-".repeat(80));
    out.push('\n');
    for s in &resp.sessions {
        let cwd = s.cwd.as_deref().unwrap_or("-");
        let idle = s.idle_secs.map(|i| i.to_string()).unwrap_or_else(|| "-".to_string());
        out.push_str(&format!("{:<36} {:<30} {:<10}\n", s.id, cwd, idle));
    }
    Ok(out)
}
