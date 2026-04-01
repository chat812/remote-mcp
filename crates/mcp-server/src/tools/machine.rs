use crate::db::{Capabilities, Db, Machine};
use crate::error::RemoteExecError;
use crate::transport::{dispatch_exec, CircuitBreakers};
use anyhow::Result;
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

pub async fn machine_add(
    db: &Db,
    label: String,
    host: String,
    port: Option<i64>,
    os: Option<String>,
    transport: Option<String>,
    ssh_user: Option<String>,
    ssh_key_path: Option<String>,
    ssh_password: Option<String>,
    agent_url: Option<String>,
    agent_token: Option<String>,
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let m = Machine {
        id: id.clone(),
        label: label.clone(),
        host,
        port: port.unwrap_or(22),
        os: os.unwrap_or_else(|| "linux".to_string()),
        transport: transport.unwrap_or_else(|| "ssh".to_string()),
        ssh_user,
        ssh_key_path,
        ssh_password,
        agent_url,
        agent_token,
        capabilities: None,
        last_seen: None,
        status: "unknown".to_string(),
        created_at: Utc::now().timestamp(),
    };
    db.upsert(&m)?;
    Ok(format!("Machine '{}' registered with id={}", label, id))
}

pub async fn machine_list(db: &Db) -> Result<String> {
    let machines = db.list()?;
    if machines.is_empty() {
        return Ok("No machines registered. Use machine_add to register one.".to_string());
    }
    let mut out = String::from("Registered machines:\n");
    out.push_str(&format!(
        "{:<36} {:<20} {:<20} {:<10} {:<12} {:<10}\n",
        "ID", "Label", "Host", "Transport", "Status", "OS"
    ));
    out.push_str(&"-".repeat(110));
    out.push('\n');
    for m in &machines {
        let last_seen = m.last_seen.map(|t| {
            let now = Utc::now().timestamp();
            format!("{}s ago", now - t)
        }).unwrap_or_else(|| "never".to_string());
        out.push_str(&format!(
            "{:<36} {:<20} {:<20} {:<10} {:<12} {:<10}\n",
            m.id, m.label, format!("{}:{}", m.host, m.port), m.transport, m.status, m.os
        ));
    }
    Ok(out)
}

pub async fn machine_remove(db: &Db, id: &str) -> Result<String> {
    if db.delete(id)? {
        Ok(format!("Machine '{}' removed.", id))
    } else {
        Err(RemoteExecError::MachineNotFound(id.to_string()).into())
    }
}

pub async fn machine_test(
    db: &Db,
    id: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db
        .get(id)?
        .ok_or_else(|| RemoteExecError::MachineNotFound(id.to_string()))?;

    let start = std::time::Instant::now();
    let result = dispatch_exec(&machine, "echo ok", 10, &circuits).await;
    let elapsed = start.elapsed().as_millis();

    match result {
        Ok(r) if r.stdout.trim() == "ok" || r.exit_code == 0 => {
            let now = chrono::Utc::now().timestamp();
            db.update_heartbeat(id, "online", now)?;
            Ok(format!(
                "Machine '{}' is reachable. RTT: {}ms",
                machine.label, elapsed
            ))
        }
        Ok(r) => {
            let now = chrono::Utc::now().timestamp();
            db.update_heartbeat(id, "online", now)?;
            Ok(format!(
                "Machine '{}' responded (exit_code={}). RTT: {}ms\nstdout: {}\nstderr: {}",
                machine.label, r.exit_code, elapsed, r.stdout, r.stderr
            ))
        }
        Err(e) => {
            let now = chrono::Utc::now().timestamp();
            db.update_heartbeat(id, "unreachable", now)?;
            Err(e.into())
        }
    }
}

/// Parse a `remcp://` URI and register the machine.
///
/// URI format (produced by `agent init` and the install scripts):
///   remcp://<host>:<port>?token=<tok>[&label=<label>][&via=agent|ssh|agent+ssh]
///
/// All query params are optional — defaults are applied when absent.
pub async fn machine_connect(db: &Db, uri: &str) -> Result<String> {
    use anyhow::bail;

    let uri = uri.trim();

    // Strip the scheme — accept both "remcp://" and plain "host:port?..."
    let rest = uri
        .strip_prefix("remcp://")
        .unwrap_or(uri);

    // Split authority (host:port) from query string
    let (authority, query) = match rest.split_once('?') {
        Some((a, q)) => (a, q),
        None => (rest, ""),
    };

    // Parse host and optional port
    let (host, port) = if let Some((h, p)) = authority.split_once(':') {
        let port: i64 = p.parse().map_err(|_| anyhow::anyhow!("invalid port '{}'", p))?;
        (h.to_string(), port)
    } else if authority.is_empty() {
        bail!("URI must contain a host. Expected: remcp://<host>:<port>?token=<token>");
    } else {
        (authority.to_string(), 8765)
    };

    // Parse query params
    let mut token = String::new();
    let mut label = String::new();
    let mut via = "agent".to_string();

    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = urlenc_decode(v);
        match k {
            "token"  => token = v,
            "label"  => label = v,
            "via"    => via = v,
            _        => {} // ignore unknown params
        }
    }

    if token.is_empty() {
        bail!("URI missing required 'token' query parameter. Expected: remcp://{}:{}?token=<secret>", host, port);
    }

    if label.is_empty() {
        label = format!("{}:{}", host, port);
    }

    let agent_url = format!("http://{}:{}", host, port);

    machine_add(
        db,
        label,
        host,
        Some(port),
        None, // os — will be filled by heartbeat capability fetch
        Some(via),
        None, // ssh_user
        None, // ssh_key_path
        None, // ssh_password
        Some(agent_url),
        Some(token),
    ).await
}

fn urlenc_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i+1..i+3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}
