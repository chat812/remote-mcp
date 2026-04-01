use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{dispatch_exec, CircuitBreakers};
use anyhow::Result;
use std::sync::Arc;

pub async fn fleet_exec(
    db: &Db,
    machine_ids: Vec<String>,
    command: &str,
    timeout_secs: Option<u64>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machines: Vec<_> = machine_ids
        .iter()
        .filter_map(|id| db.get(id).ok().flatten())
        .collect();

    if machines.is_empty() {
        return Err(anyhow::anyhow!("No valid machines found"));
    }

    let timeout = timeout_secs.unwrap_or(60);
    let mut handles = Vec::new();

    for machine in machines {
        let cmd = command.to_string();
        let circuits = circuits.clone();
        handles.push(tokio::spawn(async move {
            let result = dispatch_exec(&machine, &cmd, timeout, &circuits).await;
            (machine.label.clone(), result)
        }));
    }

    let mut out = String::new();
    for handle in handles {
        match handle.await {
            Ok((label, Ok(r))) => {
                out.push_str(&format!("=== {} ===\n", label));
                out.push_str(&r.stdout);
                if !r.stderr.is_empty() {
                    out.push_str("[stderr] ");
                    out.push_str(&r.stderr);
                }
                out.push('\n');
            }
            Ok((label, Err(e))) => {
                out.push_str(&format!("=== {} === ERROR: {}\n", label, e));
            }
            Err(e) => {
                out.push_str(&format!("=== task panicked: {} ===\n", e));
            }
        }
    }

    Ok(crate::tools::paginate(out, None))
}

pub async fn fleet_ls(
    db: &Db,
    machine_ids: Vec<String>,
    path: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    fleet_exec(db, machine_ids, &format!("ls -la {}", path), None, circuits).await
}

pub async fn fleet_upload(
    db: &Db,
    machine_ids: Vec<String>,
    local_path: &str,
    remote_path: &str,
) -> Result<String> {
    let content = std::fs::read(local_path)
        .map_err(|e| anyhow::anyhow!("Failed to read local file: {}", e))?;

    let machines: Vec<_> = machine_ids
        .iter()
        .filter_map(|id| db.get(id).ok().flatten())
        .collect();

    if machines.is_empty() {
        return Err(anyhow::anyhow!("No valid machines found"));
    }

    let mut out = String::new();
    for machine in &machines {
        if let Some(agent_url) = &machine.agent_url {
            let token = machine.agent_token.as_deref().unwrap_or("");
            match crate::auth::sign_request(token, "POST", "/file/upload", &content) {
                Ok(signed) => {
                    let client = reqwest::Client::new();
                    let form = reqwest::multipart::Form::new()
                        .part("file", reqwest::multipart::Part::bytes(content.clone()).file_name(remote_path.to_string()))
                        .text("path", remote_path.to_string());

                    match client
                        .post(&format!("{}/file/upload", agent_url.trim_end_matches('/')))
                        .header("X-Agent-Timestamp", &signed.timestamp)
                        .header("X-Agent-Signature", &signed.signature)
                        .multipart(form)
                        .timeout(std::time::Duration::from_secs(600))
                        .send()
                        .await
                    {
                        Ok(resp) if resp.status().is_success() => {
                            out.push_str(&format!("{}: uploaded OK\n", machine.label));
                        }
                        Ok(resp) => {
                            out.push_str(&format!("{}: upload failed (HTTP {})\n", machine.label, resp.status()));
                        }
                        Err(e) => {
                            out.push_str(&format!("{}: upload error: {}\n", machine.label, e));
                        }
                    }
                }
                Err(e) => {
                    out.push_str(&format!("{}: sign error: {}\n", machine.label, e));
                }
            }
        } else {
            out.push_str(&format!("{}: no agent_url, skipping\n", machine.label));
        }
    }

    Ok(out)
}
