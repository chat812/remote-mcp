use anyhow::{anyhow, Result};
use bytes::Bytes;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use strip_ansi_escapes;
use tokio::sync::oneshot;
use tracing::{debug, warn};
use uuid::Uuid;

pub struct Session {
    pub id: String,
    pub cwd: Option<String>,
    pub last_active: Arc<Mutex<Instant>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader: Arc<Mutex<Box<dyn Read + Send>>>,
}

pub type SessionStore = Arc<dashmap::DashMap<String, Arc<Session>>>;

pub fn new_store() -> SessionStore {
    let store: SessionStore = Arc::new(dashmap::DashMap::new());
    start_idle_reaper(store.clone());
    store
}

fn start_idle_reaper(store: SessionStore) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            store.retain(|_, session| {
                let idle = session.last_active.lock().unwrap().elapsed().as_secs();
                idle < 1800 // 30 min
            });
        }
    });
}

impl Session {
    pub fn open(workdir: Option<String>, shell: Option<String>) -> Result<Arc<Self>> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("Failed to open PTY: {}", e))?;

        let shell_cmd = shell.unwrap_or_else(|| {
            if cfg!(windows) {
                "cmd.exe".to_string()
            } else {
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
            }
        });

        let mut cmd = CommandBuilder::new(&shell_cmd);
        if let Some(ref wd) = workdir {
            cmd.cwd(wd);
        }

        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow!("Failed to spawn shell: {}", e))?;

        let writer = pair.master.take_writer()
            .map_err(|e| anyhow!("Failed to get PTY writer: {}", e))?;
        let reader = pair.master.try_clone_reader()
            .map_err(|e| anyhow!("Failed to get PTY reader: {}", e))?;

        Ok(Arc::new(Self {
            id: Uuid::new_v4().to_string(),
            cwd: workdir,
            last_active: Arc::new(Mutex::new(Instant::now())),
            writer: Arc::new(Mutex::new(writer)),
            reader: Arc::new(Mutex::new(reader)),
        }))
    }

    pub async fn exec(&self, command: &str, timeout_secs: u64) -> Result<(String, i32)> {
        let sentinel = format!("__DONE_{}__", Uuid::new_v4());
        let cmd_line = format!(
            "{}; echo 'EXIT:$?'; echo '{}'\n",
            command, sentinel
        );

        // Update last active
        *self.last_active.lock().unwrap() = Instant::now();

        // Write command
        {
            let mut writer = self.writer.lock().unwrap();
            writer.write_all(cmd_line.as_bytes())?;
            writer.flush()?;
        }

        // Read until sentinel (blocking in a thread to avoid blocking tokio)
        let reader = self.reader.clone();
        let sentinel_clone = sentinel.clone();

        let (tx, rx) = tokio::sync::oneshot::channel();

        std::thread::spawn(move || {
            let result = read_until_sentinel(&reader, &sentinel_clone);
            let _ = tx.send(result);
        });

        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(timeout_secs),
            rx,
        )
        .await
        .map_err(|_| anyhow!("Session exec timed out after {}s", timeout_secs))?
        .map_err(|e| anyhow!("Session exec channel error: {}", e))??;

        // Update last active
        *self.last_active.lock().unwrap() = Instant::now();

        Ok(result)
    }

    pub fn idle_secs(&self) -> u64 {
        self.last_active.lock().unwrap().elapsed().as_secs()
    }
}

fn read_until_sentinel(
    reader: &Arc<Mutex<Box<dyn Read + Send>>>,
    sentinel: &str,
) -> Result<(String, i32)> {
    let mut output = String::new();
    let mut exit_code = 0i32;
    let mut buf = [0u8; 4096];

    loop {
        let n = {
            let mut r = reader.lock().unwrap();
            r.read(&mut buf).unwrap_or(0)
        };

        if n == 0 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
        output.push_str(&chunk);

        if output.contains(sentinel) {
            // Parse exit code
            if let Some(exit_line) = output.lines().find(|l| l.starts_with("EXIT:")) {
                let code_str = exit_line.trim_start_matches("EXIT:");
                exit_code = code_str.trim().parse().unwrap_or(0);
            }

            // Remove exit code line and sentinel
            let clean: Vec<&str> = output
                .lines()
                .filter(|l| !l.starts_with("EXIT:") && !l.contains(sentinel))
                .collect();
            let raw = clean.join("\n");

            // Strip ANSI
            let stripped_bytes = strip_ansi_escapes::strip(raw.as_bytes());
            let stripped = String::from_utf8_lossy(&stripped_bytes).into_owned();

            return Ok((stripped, exit_code));
        }
    }
}
