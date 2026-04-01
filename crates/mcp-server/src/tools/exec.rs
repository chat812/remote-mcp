use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::{agent, dispatch_exec, CircuitBreakers};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize)]
struct JobStartRequest<'a> {
    command: &'a str,
    workdir: Option<&'a str>,
}

#[derive(Deserialize)]
struct JobStartResponse {
    job_id: String,
}

#[derive(Deserialize)]
struct JobStatusResponse {
    job_id: String,
    status: String,
    exit_code: Option<i32>,
    started_at: Option<i64>,
    finished_at: Option<i64>,
}

#[derive(Serialize)]
struct JobLogsRequest {
    tail: Option<usize>,
    stream: Option<String>,
}

#[derive(Deserialize)]
struct JobLogsResponse {
    stdout: Option<String>,
    stderr: Option<String>,
}

#[derive(Deserialize)]
struct JobListResponse {
    jobs: Vec<JobStatusResponse>,
}

pub async fn exec(
    db: &Db,
    machine_id: &str,
    command: &str,
    workdir: Option<&str>,
    timeout_secs: Option<u64>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db
        .get(machine_id)?
        .ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    let timeout = timeout_secs.unwrap_or(60);
    let cmd = if let Some(wd) = workdir {
        format!("cd {} && {}", shell_escape(wd), command)
    } else {
        command.to_string()
    };

    let result = dispatch_exec(&machine, &cmd, timeout, &circuits).await?;

    let mut out = String::new();
    if !result.stdout.is_empty() {
        out.push_str(&result.stdout);
    }
    if !result.stderr.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[stderr]\n");
        out.push_str(&result.stderr);
    }
    if result.exit_code != 0 {
        out.push_str(&format!("\n[exit_code: {}]", result.exit_code));
    }

    Ok(crate::tools::paginate(out, None))
}

pub async fn job_start(
    db: &Db,
    machine_id: &str,
    command: &str,
    workdir: Option<&str>,
    circuits: Arc<CircuitBreakers>,
) -> Result<String> {
    let machine = db
        .get(machine_id)?
        .ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired {
            tool: "job_start".to_string(),
        }
        .into());
    }

    circuits.check(&machine.id, &machine.label).await?;

    let req = JobStartRequest {
        command,
        workdir,
    };

    let resp: JobStartResponse =
        agent::agent_post_json(&machine, "/job/start", &req, 10).await?;

    Ok(format!("Job started: {}", resp.job_id))
}

pub async fn job_status(
    db: &Db,
    machine_id: &str,
    job_id: &str,
) -> Result<String> {
    let machine = db
        .get(machine_id)?
        .ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired {
            tool: "job_status".to_string(),
        }
        .into());
    }

    let resp: JobStatusResponse =
        agent::agent_get_json(&machine, &format!("/job/{}", job_id), 10).await?;

    let mut out = format!("Job {} — status: {}\n", resp.job_id, resp.status);
    if let Some(ec) = resp.exit_code {
        out.push_str(&format!("Exit code: {}\n", ec));
    }
    if let Some(s) = resp.started_at {
        out.push_str(&format!("Started: {}\n", s));
    }
    if let Some(f) = resp.finished_at {
        out.push_str(&format!("Finished: {}\n", f));
    }
    Ok(out)
}

pub async fn job_logs(
    db: &Db,
    machine_id: &str,
    job_id: &str,
    tail: Option<usize>,
    stream: Option<String>,
) -> Result<String> {
    let machine = db
        .get(machine_id)?
        .ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired {
            tool: "job_logs".to_string(),
        }
        .into());
    }

    let mut path = format!("/job/{}/logs", job_id);
    let mut params = Vec::new();
    if let Some(t) = tail {
        params.push(format!("tail={}", t));
    }
    if let Some(s) = &stream {
        params.push(format!("stream={}", s));
    }
    if !params.is_empty() {
        path = format!("{}?{}", path, params.join("&"));
    }

    let resp: JobLogsResponse = agent::agent_get_json(&machine, &path, 10).await?;

    let mut out = String::new();
    if let Some(stdout) = resp.stdout {
        out.push_str(&stdout);
    }
    if let Some(stderr) = resp.stderr {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[stderr]\n");
        out.push_str(&stderr);
    }

    Ok(crate::tools::paginate(out, None))
}

pub async fn job_kill(
    db: &Db,
    machine_id: &str,
    job_id: &str,
) -> Result<String> {
    let machine = db
        .get(machine_id)?
        .ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired {
            tool: "job_kill".to_string(),
        }
        .into());
    }

    let _: serde_json::Value =
        agent::agent_post_json(&machine, &format!("/job/{}/kill", job_id), &serde_json::json!({}), 10).await?;

    Ok(format!("Kill signal sent to job {}", job_id))
}

pub async fn job_list(
    db: &Db,
    machine_id: &str,
) -> Result<String> {
    let machine = db
        .get(machine_id)?
        .ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;

    if machine.agent_url.is_none() {
        return Err(RemoteExecError::AgentRequired {
            tool: "job_list".to_string(),
        }
        .into());
    }

    let resp: JobListResponse = agent::agent_get_json(&machine, "/jobs", 10).await?;

    if resp.jobs.is_empty() {
        return Ok("No jobs found.".to_string());
    }

    let mut out = format!("{:<36} {:<12} {:<10}\n", "Job ID", "Status", "Exit Code");
    out.push_str(&"-".repeat(60));
    out.push('\n');
    for job in &resp.jobs {
        let ec = job.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "-".to_string());
        out.push_str(&format!("{:<36} {:<12} {:<10}\n", job.job_id, job.status, ec));
    }
    Ok(out)
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}
