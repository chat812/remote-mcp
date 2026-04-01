use crate::db::Machine;
use crate::error::RemoteExecError;
use crate::transport::ExecResult;
use async_trait::async_trait;
use dashmap::DashMap;
use russh::client::{self, Handle};
use russh::keys::key::KeyPair;
use russh::{Channel, ChannelMsg};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::timeout;
use tracing::{debug, warn};

const MAX_CONNECTIONS: usize = 4;
const IDLE_TIMEOUT_SECS: u64 = 600; // 10 min

struct SshClient;

#[async_trait]
impl client::Handler for SshClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true) // Accept all host keys (for simplicity; production should verify)
    }
}

struct PooledConn {
    handle: Handle<SshClient>,
    last_used: Instant,
}

pub struct SshPool {
    machine_id: String,
    host: String,
    port: u16,
    user: String,
    key_path: Option<String>,
    password: Option<String>,
    semaphore: Arc<Semaphore>,
    connections: Arc<Mutex<Vec<PooledConn>>>,
}

impl SshPool {
    pub fn new(machine: &Machine) -> Self {
        Self {
            machine_id: machine.id.clone(),
            host: machine.host.clone(),
            port: machine.port as u16,
            user: machine.ssh_user.clone().unwrap_or_else(|| "root".to_string()),
            key_path: machine.ssh_key_path.clone(),
            password: machine.ssh_password.clone(),
            semaphore: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            connections: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn connect(&self) -> Result<Handle<SshClient>, RemoteExecError> {
        let config = Arc::new(russh::client::Config {
            ..Default::default()
        });

        let handler = SshClient;
        let addr = format!("{}:{}", self.host, self.port);
        let mut handle = client::connect(config, addr, handler)
            .await
            .map_err(|e| RemoteExecError::SshError {
                machine: self.machine_id.clone(),
                reason: e.to_string(),
            })?;

        // Authenticate
        if let Some(key_path) = &self.key_path {
            let key = russh::keys::load_secret_key(key_path, None)
                .map_err(|e| RemoteExecError::SshError {
                    machine: self.machine_id.clone(),
                    reason: format!("Failed to load key {}: {}", key_path, e),
                })?;
            handle
                .authenticate_publickey(&self.user, Arc::new(key))
                .await
                .map_err(|e| RemoteExecError::SshError {
                    machine: self.machine_id.clone(),
                    reason: format!("Key auth failed: {}", e),
                })?;
        } else if let Some(pw) = &self.password {
            handle
                .authenticate_password(&self.user, pw)
                .await
                .map_err(|e| RemoteExecError::SshError {
                    machine: self.machine_id.clone(),
                    reason: format!("Password auth failed: {}", e),
                })?;
        } else {
            handle
                .authenticate_password(&self.user, "")
                .await
                .map_err(|e| RemoteExecError::SshError {
                    machine: self.machine_id.clone(),
                    reason: format!("Auth failed: {}", e),
                })?;
        }

        Ok(handle)
    }

    async fn acquire(&self) -> Result<Handle<SshClient>, RemoteExecError> {
        // Try to get an idle connection
        {
            let mut conns = self.connections.lock().await;
            while let Some(pooled) = conns.pop() {
                // Health check with echo
                // If it fails, discard and try another
                if pooled.handle.is_closed() {
                    continue;
                }
                let elapsed = pooled.last_used.elapsed().as_secs();
                if elapsed > IDLE_TIMEOUT_SECS {
                    continue;
                }
                return Ok(pooled.handle);
            }
        }
        self.connect().await
    }

    async fn release(&self, handle: Handle<SshClient>) {
        let pooled = PooledConn {
            handle,
            last_used: Instant::now(),
        };
        let mut conns = self.connections.lock().await;
        if conns.len() < MAX_CONNECTIONS {
            conns.push(pooled);
        }
        // else drop it
    }

    pub async fn exec(
        &self,
        cmd: &str,
        timeout_secs: u64,
    ) -> Result<ExecResult, RemoteExecError> {
        let _permit = self.semaphore.acquire().await.map_err(|e| RemoteExecError::SshError {
            machine: self.machine_id.clone(),
            reason: format!("Semaphore error: {}", e),
        })?;

        let handle = self.acquire().await?;

        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|e| RemoteExecError::SshError {
                machine: self.machine_id.clone(),
                reason: format!("Channel open failed: {}", e),
            })?;

        channel
            .exec(true, cmd)
            .await
            .map_err(|e| RemoteExecError::SshError {
                machine: self.machine_id.clone(),
                reason: format!("Exec failed: {}", e),
            })?;

        let result = timeout(
            Duration::from_secs(timeout_secs),
            collect_channel_output(&mut channel),
        )
        .await
        .map_err(|_| RemoteExecError::Timeout {
            machine: self.machine_id.clone(),
            after_secs: timeout_secs,
        })?
        .map_err(|e| RemoteExecError::SshError {
            machine: self.machine_id.clone(),
            reason: e.to_string(),
        })?;

        self.release(handle).await;
        Ok(result)
    }
}

async fn collect_channel_output(
    channel: &mut Channel<client::Msg>,
) -> Result<ExecResult, russh::Error> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code = 0i32;

    loop {
        let msg = channel.wait().await;
        match msg {
            Some(ChannelMsg::Data { data }) => {
                stdout.extend_from_slice(&data);
            }
            Some(ChannelMsg::ExtendedData { data, .. }) => {
                stderr.extend_from_slice(&data);
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                exit_code = exit_status as i32;
            }
            Some(ChannelMsg::Eof) | None => break,
            _ => {}
        }
    }

    Ok(ExecResult {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_code,
    })
}

pub type SshPools = DashMap<String, Arc<SshPool>>;

pub fn create_pool_store() -> Arc<SshPools> {
    let pools: Arc<SshPools> = Arc::new(DashMap::new());
    // Start background cleanup task
    let pools_clone = pools.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(300)); // 5 min
        loop {
            ticker.tick().await;
            for entry in pools_clone.iter() {
                let pool = entry.value();
                let mut conns = pool.connections.lock().await;
                let now = Instant::now();
                conns.retain(|c| {
                    now.duration_since(c.last_used).as_secs() < IDLE_TIMEOUT_SECS
                });
            }
        }
    });
    pools
}

pub async fn exec_ssh(
    machine: &Machine,
    cmd: &str,
    timeout_secs: u64,
) -> Result<ExecResult, RemoteExecError> {
    // Simple direct connection (not pooled) for now
    let pool = SshPool::new(machine);
    pool.exec(cmd, timeout_secs).await
}
