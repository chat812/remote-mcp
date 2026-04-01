use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{agent, dispatch_exec, CircuitBreakers};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub async fn fs_ls(
    db: &Db,
    machine_id: &str,
    path: &str,
    all: bool,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = format!("/fs/ls?path={}&all={}", urlenc(path), all);
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let flags = if all { "-la" } else { "-l" };
        let cmd = format!("ls {} {}", flags, shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn fs_stat(
    db: &Db,
    machine_id: &str,
    path: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = format!("/fs/stat?path={}", urlenc(path));
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let cmd = format!("stat {}", shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(r.stdout)
    }
}

pub async fn fs_find(
    db: &Db,
    machine_id: &str,
    path: &str,
    pattern: Option<&str>,
    file_type: Option<&str>,
    max_depth: Option<i32>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({
            "path": path,
            "pattern": pattern,
            "file_type": file_type,
            "max_depth": max_depth,
        });
        let result: serde_json::Value = agent::agent_post_json(&machine, "/fs/find", &req, 30).await?;
        Ok(result.to_string())
    } else {
        let mut cmd = format!("find {}", shell_escape(path));
        if let Some(d) = max_depth {
            cmd.push_str(&format!(" -maxdepth {}", d));
        }
        if let Some(t) = file_type {
            let ft = match t { "file" => "f", "dir" => "d", "symlink" => "l", other => other };
            cmd.push_str(&format!(" -type {}", ft));
        }
        if let Some(p) = pattern {
            cmd.push_str(&format!(" -name {}", shell_escape(p)));
        }
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn fs_tree(
    db: &Db,
    machine_id: &str,
    path: &str,
    max_depth: Option<i32>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let url = format!("/fs/tree?path={}&depth={}", urlenc(path), max_depth.unwrap_or(3));
        let result: serde_json::Value = agent::agent_get_json(&machine, &url, 30).await?;
        Ok(result.to_string())
    } else {
        let depth_arg = max_depth.map(|d| format!("-L {}", d)).unwrap_or_else(|| "-L 3".to_string());
        let cmd = format!("tree {} {} 2>/dev/null || find {} -maxdepth {} | sort",
            depth_arg, shell_escape(path),
            shell_escape(path), max_depth.unwrap_or(3));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn fs_mkdir(
    db: &Db,
    machine_id: &str,
    path: &str,
    parents: bool,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "path": path, "parents": parents });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/fs/mkdir", &req, 10).await?;
        Ok(format!("Directory created: {}", path))
    } else {
        let flags = if parents { "-p" } else { "" };
        let cmd = format!("mkdir {} {}", flags, shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 10, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("mkdir failed: {}", r.stderr));
        }
        Ok(format!("Directory created: {}", path))
    }
}

pub async fn fs_rm(
    db: &Db,
    machine_id: &str,
    path: &str,
    recursive: bool,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "path": path, "recursive": recursive });
        let signed = crate::auth::sign_request(
            machine.agent_token.as_deref().unwrap_or(""),
            "DELETE",
            &format!("/fs/rm?path={}&recursive={}", urlenc(path), recursive),
            b"",
        )?;
        let client = reqwest::Client::new();
        let url = format!("{}/fs/rm?path={}&recursive={}", machine.agent_url.as_ref().unwrap().trim_end_matches('/'), urlenc(path), recursive);
        let resp = client.delete(&url)
            .header("X-Agent-Timestamp", &signed.timestamp)
            .header("X-Agent-Signature", &signed.signature)
            .timeout(std::time::Duration::from_secs(30))
            .send().await?;
        if !resp.status().is_success() {
            let s = resp.status().as_u16();
            let b = resp.text().await.unwrap_or_default();
            return Err(RemoteExecError::AgentHttpError { machine: machine.id, status: s, body: b }.into());
        }
        Ok(format!("Removed: {}", path))
    } else {
        let flags = if recursive { "-rf" } else { "-f" };
        let cmd = format!("rm {} {}", flags, shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("rm failed: {}", r.stderr));
        }
        Ok(format!("Removed: {}", path))
    }
}

pub async fn fs_mv(
    db: &Db,
    machine_id: &str,
    src: &str,
    dst: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "src": src, "dst": dst });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/fs/mv", &req, 30).await?;
        Ok(format!("Moved {} -> {}", src, dst))
    } else {
        let cmd = format!("mv {} {}", shell_escape(src), shell_escape(dst));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("mv failed: {}", r.stderr));
        }
        Ok(format!("Moved {} -> {}", src, dst))
    }
}

pub async fn fs_cp(
    db: &Db,
    machine_id: &str,
    src: &str,
    dst: &str,
    recursive: bool,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "src": src, "dst": dst, "recursive": recursive });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/fs/cp", &req, 60).await?;
        Ok(format!("Copied {} -> {}", src, dst))
    } else {
        let flags = if recursive { "-r" } else { "" };
        let cmd = format!("cp {} {} {}", flags, shell_escape(src), shell_escape(dst));
        let r = dispatch_exec(&machine, &cmd, 60, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("cp failed: {}", r.stderr));
        }
        Ok(format!("Copied {} -> {}", src, dst))
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
