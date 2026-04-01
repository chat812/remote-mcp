use crate::auth::sign_request;
use crate::db::Machine;
use crate::error::RemoteExecError;
use crate::transport::ExecResult;
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::time::Duration;

fn make_client() -> Client {
    ClientBuilder::new()
        .connect_timeout(Duration::from_secs(5))
        .tcp_keepalive(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(4)
        .build()
        .expect("agent client build")
}

/// Shared HTTP client — one per process, connection pool reused across all requests.
static SHARED_CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();

fn shared_client() -> &'static Client {
    SHARED_CLIENT.get_or_init(make_client)
}

pub struct AgentClient {
    base_url: String,
    token: String,
    machine_id: String,
}

impl AgentClient {
    pub fn new(machine: &Machine) -> Option<Self> {
        let base_url = machine.agent_url.clone()?;
        let token = machine.agent_token.clone().unwrap_or_default();
        Some(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            machine_id: machine.id.clone(),
        })
    }

    async fn signed_post<T: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &T,
        timeout_secs: u64,
    ) -> Result<R, RemoteExecError> {
        let body_bytes = serde_json::to_vec(body).map_err(|e| RemoteExecError::AgentHttpError {
            machine: self.machine_id.clone(),
            status: 0,
            body: e.to_string(),
        })?;

        let signed = sign_request(&self.token, "POST", path, &body_bytes).map_err(|e| {
            RemoteExecError::AgentHttpError {
                machine: self.machine_id.clone(),
                status: 0,
                body: e.to_string(),
            }
        })?;

        let url = format!("{}{}", self.base_url, path);
        let resp = shared_client()
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Agent-Timestamp", &signed.timestamp)
            .header("X-Agent-Signature", &signed.signature)
            .body(body_bytes)
            .timeout(Duration::from_secs(timeout_secs))
            .send()
            .await
            .map_err(|e| RemoteExecError::AgentHttpError {
                machine: self.machine_id.clone(),
                status: 0,
                body: e.to_string(),
            })?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(RemoteExecError::AgentHttpError {
                machine: self.machine_id.clone(),
                status,
                body,
            });
        }

        resp.json::<R>().await.map_err(|e| RemoteExecError::AgentHttpError {
            machine: self.machine_id.clone(),
            status,
            body: e.to_string(),
        })
    }

    async fn signed_get<R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        timeout_secs: u64,
    ) -> Result<R, RemoteExecError> {
        let signed = sign_request(&self.token, "GET", path, b"").map_err(|e| {
            RemoteExecError::AgentHttpError {
                machine: self.machine_id.clone(),
                status: 0,
                body: e.to_string(),
            }
        })?;

        let url = format!("{}{}", self.base_url, path);
        let resp = shared_client()
            .get(&url)
            .header("X-Agent-Timestamp", &signed.timestamp)
            .header("X-Agent-Signature", &signed.signature)
            .timeout(Duration::from_secs(timeout_secs))
            .send()
            .await
            .map_err(|e| RemoteExecError::AgentHttpError {
                machine: self.machine_id.clone(),
                status: 0,
                body: e.to_string(),
            })?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(RemoteExecError::AgentHttpError {
                machine: self.machine_id.clone(),
                status,
                body,
            });
        }

        resp.json::<R>().await.map_err(|e| RemoteExecError::AgentHttpError {
            machine: self.machine_id.clone(),
            status,
            body: e.to_string(),
        })
    }
}

#[derive(Serialize)]
struct ExecRequest<'a> {
    command: &'a str,
    workdir: Option<&'a str>,
    timeout_secs: u64,
}

#[derive(Deserialize)]
struct ExecResponse {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

pub async fn exec_agent(
    machine: &Machine,
    cmd: &str,
    timeout_secs: u64,
) -> Result<ExecResult, RemoteExecError> {
    let client = AgentClient::new(machine).ok_or_else(|| RemoteExecError::ConnectionFailed {
        machine: machine.label.clone(),
        reason: "No agent_url configured".to_string(),
    })?;

    let req = ExecRequest {
        command: cmd,
        workdir: None,
        timeout_secs,
    };

    // Add a 10s buffer over the requested command timeout for network overhead.
    let http_timeout = timeout_secs.saturating_add(10).max(30);
    let resp: ExecResponse = client.signed_post("/exec", &req, http_timeout).await?;
    Ok(ExecResult {
        stdout: resp.stdout,
        stderr: resp.stderr,
        exit_code: resp.exit_code,
    })
}

pub async fn agent_get_text(
    machine: &Machine,
    path: &str,
    timeout_secs: u64,
) -> Result<String, RemoteExecError> {
    let client = AgentClient::new(machine).ok_or_else(|| RemoteExecError::ConnectionFailed {
        machine: machine.label.clone(),
        reason: "No agent_url configured".to_string(),
    })?;

    let signed = sign_request(&client.token, "GET", path, b"").map_err(|e| {
        RemoteExecError::AgentHttpError {
            machine: client.machine_id.clone(),
            status: 0,
            body: e.to_string(),
        }
    })?;

    let url = format!("{}{}", client.base_url, path);
    let resp = shared_client()
        .get(&url)
        .header("X-Agent-Timestamp", &signed.timestamp)
        .header("X-Agent-Signature", &signed.signature)
        .timeout(Duration::from_secs(timeout_secs))
        .send()
        .await
        .map_err(|e| RemoteExecError::AgentHttpError {
            machine: client.machine_id.clone(),
            status: 0,
            body: e.to_string(),
        })?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(RemoteExecError::AgentHttpError {
            machine: client.machine_id.clone(),
            status,
            body,
        });
    }

    resp.text().await.map_err(|e| RemoteExecError::AgentHttpError {
        machine: client.machine_id.clone(),
        status,
        body: e.to_string(),
    })
}

pub async fn agent_post_json<T: Serialize, R: for<'de> Deserialize<'de>>(
    machine: &Machine,
    path: &str,
    body: &T,
    timeout_secs: u64,
) -> Result<R, RemoteExecError> {
    let client = AgentClient::new(machine).ok_or_else(|| RemoteExecError::ConnectionFailed {
        machine: machine.label.clone(),
        reason: "No agent_url configured".to_string(),
    })?;
    client.signed_post(path, body, timeout_secs).await
}

pub async fn agent_get_json<R: for<'de> Deserialize<'de>>(
    machine: &Machine,
    path: &str,
    timeout_secs: u64,
) -> Result<R, RemoteExecError> {
    let client = AgentClient::new(machine).ok_or_else(|| RemoteExecError::ConnectionFailed {
        machine: machine.label.clone(),
        reason: "No agent_url configured".to_string(),
    })?;
    client.signed_get(path, timeout_secs).await
}
