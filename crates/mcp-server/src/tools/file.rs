use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{agent, dispatch_exec, CircuitBreakers, ExecResult};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

fn require_agent(machine: &crate::db::Machine, tool: &str) -> Result<()> {
    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired {
            tool: tool.to_string(),
        }
        .into());
    }
    Ok(())
}

#[derive(Serialize)]
struct FileWriteReq<'a> {
    path: &'a str,
    content: &'a str,
    mode: Option<&'a str>,
}

#[derive(Serialize)]
struct FileReadReq<'a> {
    path: &'a str,
}

#[derive(Deserialize)]
struct FileReadResp {
    content: String,
    size: u64,
}

#[derive(Serialize)]
struct FileStrReplaceReq<'a> {
    path: &'a str,
    old_str: &'a str,
    new_str: &'a str,
}

#[derive(Serialize)]
struct FilePatchReq<'a> {
    path: &'a str,
    unified_diff: &'a str,
}

#[derive(Serialize)]
struct FileInsertReq<'a> {
    path: &'a str,
    line: usize,
    content: &'a str,
}

#[derive(Serialize)]
struct FileDeleteLinesReq<'a> {
    path: &'a str,
    start_line: usize,
    end_line: usize,
}

pub async fn file_write(
    db: &Db,
    machine_id: &str,
    path: &str,
    content: &str,
    mode: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = FileWriteReq { path, content, mode };
        let _: serde_json::Value = agent::agent_post_json(&machine, "/file/write", &req, 30).await?;
        Ok(format!("File written: {}", path))
    } else {
        // SSH fallback: use cat with heredoc
        let encoded = base64_encode(content.as_bytes());
        let cmd = format!("echo '{}' | base64 -d > {}", encoded, shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("Failed to write file: {}", r.stderr));
        }
        Ok(format!("File written: {}", path))
    }
}

pub async fn file_read(
    db: &Db,
    machine_id: &str,
    path: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let resp: FileReadResp = agent::agent_get_json(&machine, &format!("/file/read?path={}", urlenc(path)), 30).await?;
        Ok(crate::tools::paginate(resp.content, Some(resp.size as usize)))
    } else {
        let cmd = format!("cat {}", shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("Failed to read file: {}", r.stderr));
        }
        Ok(crate::tools::paginate(r.stdout, None))
    }
}

pub async fn file_str_replace(
    db: &Db,
    machine_id: &str,
    path: &str,
    old_str: &str,
    new_str: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = FileStrReplaceReq { path, old_str, new_str };
        let _: serde_json::Value = agent::agent_post_json(&machine, "/file/str-replace", &req, 30).await?;
        Ok(format!("Replacement applied to {}", path))
    } else {
        // SSH fallback: use python or sed
        let cmd = format!(
            "python3 -c \"
import sys
data = open({},'r').read()
old = sys.argv[1]
new = sys.argv[2]
if old not in data:
    print('ERROR: old_str not found', file=sys.stderr)
    sys.exit(1)
open({},'w').write(data.replace(old, new, 1))
print('done')
\" '{}' '{}'",
            shell_escape(path), shell_escape(path),
            old_str.replace('\'', "'\"'\"'"),
            new_str.replace('\'', "'\"'\"'")
        );
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("str_replace failed: {}", r.stderr));
        }
        Ok(format!("Replacement applied to {}", path))
    }
}

pub async fn file_patch(
    db: &Db,
    machine_id: &str,
    path: &str,
    unified_diff: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = FilePatchReq { path, unified_diff };
        let _: serde_json::Value = agent::agent_post_json(&machine, "/file/patch", &req, 30).await?;
        Ok(format!("Patch applied to {}", path))
    } else {
        let encoded = base64_encode(unified_diff.as_bytes());
        let cmd = format!(
            "echo '{}' | base64 -d | patch {} -",
            encoded,
            shell_escape(path)
        );
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("patch failed: {}", r.stderr));
        }
        Ok(format!("Patch applied to {}", path))
    }
}

pub async fn file_insert(
    db: &Db,
    machine_id: &str,
    path: &str,
    line: usize,
    content: &str,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "path": path, "line": line, "content": content });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/file/insert", &req, 30).await?;
        Ok(format!("Content inserted at line {} in {}", line, path))
    } else {
        let encoded = base64_encode(content.as_bytes());
        let cmd = format!(
            "python3 -c \"
import sys, base64
lines = open({p}, 'r').readlines()
ins = base64.b64decode('{enc}').decode()
if not ins.endswith('\\n'):
    ins += '\\n'
lines.insert({line}, ins)
open({p}, 'w').writelines(lines)
print('done')
\"",
            p = shell_escape(path),
            enc = encoded,
            line = line
        );
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("insert failed: {}", r.stderr));
        }
        Ok(format!("Content inserted at line {} in {}", line, path))
    }
}

pub async fn file_delete_lines(
    db: &Db,
    machine_id: &str,
    path: &str,
    start_line: usize,
    end_line: usize,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_some() {
        let req = serde_json::json!({ "path": path, "start_line": start_line, "end_line": end_line });
        let _: serde_json::Value = agent::agent_post_json(&machine, "/file/delete-lines", &req, 30).await?;
        Ok(format!("Lines {}-{} deleted from {}", start_line, end_line, path))
    } else {
        let cmd = format!("sed -i '{},{}d' {}", start_line, end_line, shell_escape(path));
        let r = dispatch_exec(&machine, &cmd, 30, &circuits).await?;
        if r.exit_code != 0 {
            return Err(anyhow::anyhow!("delete_lines failed: {}", r.stderr));
        }
        Ok(format!("Lines {}-{} deleted from {}", start_line, end_line, path))
    }
}

pub async fn file_upload(
    db: &Db,
    machine_id: &str,
    local_path: &str,
    remote_path: &str,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;
    require_agent(&machine, "file_upload")?;

    let content = std::fs::read(local_path)?;
    let agent_url = machine.agent_url.as_ref().unwrap();
    let token = machine.agent_token.as_deref().unwrap_or("");

    let signed = crate::auth::sign_request(token, "POST", "/file/upload", &content)?;

    let client = reqwest::Client::new();
    let form = reqwest::multipart::Form::new()
        .part("file", reqwest::multipart::Part::bytes(content).file_name(remote_path.to_string()))
        .text("path", remote_path.to_string());

    let resp = client
        .post(&format!("{}/file/upload", agent_url.trim_end_matches('/')))
        .header("X-Agent-Timestamp", &signed.timestamp)
        .header("X-Agent-Signature", &signed.signature)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(RemoteExecError::AgentHttpError {
            machine: machine.id,
            status,
            body,
        }.into());
    }

    Ok(format!("Uploaded {} -> {}", local_path, remote_path))
}

pub async fn file_download(
    db: &Db,
    machine_id: &str,
    remote_path: &str,
    local_path: &str,
) -> Result<String> {
    let machine = db.get(machine_id)?.ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;
    require_agent(&machine, "file_download")?;

    let agent_url = machine.agent_url.as_ref().unwrap();
    let token = machine.agent_token.as_deref().unwrap_or("");
    let path = format!("/file/download?path={}", urlenc(remote_path));

    let signed = crate::auth::sign_request(token, "GET", &path, b"")?;

    let client = reqwest::Client::new();
    let resp = client
        .get(&format!("{}{}", agent_url.trim_end_matches('/'), path))
        .header("X-Agent-Timestamp", &signed.timestamp)
        .header("X-Agent-Signature", &signed.signature)
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(RemoteExecError::AgentHttpError {
            machine: machine.id,
            status,
            body,
        }.into());
    }

    let bytes = resp.bytes().await?;
    std::fs::write(local_path, &bytes)?;

    Ok(format!("Downloaded {} -> {} ({} bytes)", remote_path, local_path, bytes.len()))
}

fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 2 < data.len() {
        let b0 = data[i] as usize;
        let b1 = data[i + 1] as usize;
        let b2 = data[i + 2] as usize;
        out.push(alphabet[b0 >> 2] as char);
        out.push(alphabet[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(alphabet[((b1 & 0xf) << 2) | (b2 >> 6)] as char);
        out.push(alphabet[b2 & 0x3f] as char);
        i += 3;
    }
    let rem = data.len() - i;
    if rem == 1 {
        let b0 = data[i] as usize;
        out.push(alphabet[b0 >> 2] as char);
        out.push(alphabet[(b0 & 3) << 4] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let b0 = data[i] as usize;
        let b1 = data[i + 1] as usize;
        out.push(alphabet[b0 >> 2] as char);
        out.push(alphabet[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(alphabet[(b1 & 0xf) << 2] as char);
        out.push('=');
    }
    out
}

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
