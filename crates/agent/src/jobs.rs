use anyhow::Result;
use bytes::Bytes;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Notify;
use tracing::{debug, warn};
use uuid::Uuid;

pub const MAX_LINES: usize = 10_000;
pub const MAX_BUFFER_BYTES: usize = 50 * 1024 * 1024; // 50 MB

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Finished,
    Failed,
    Killed,
}

#[derive(Debug)]
pub struct StreamBuffer {
    pub lines: Mutex<VecDeque<String>>,
    pub bytes_total: AtomicU64,
    pub bytes_dropped: AtomicU64,
}

impl StreamBuffer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            lines: Mutex::new(VecDeque::new()),
            bytes_total: AtomicU64::new(0),
            bytes_dropped: AtomicU64::new(0),
        })
    }

    pub fn push(&self, line: String) {
        let len = line.len() as u64;
        self.bytes_total.fetch_add(len, Ordering::Relaxed);

        let mut q = self.lines.lock().unwrap();
        let total_bytes: usize = q.iter().map(|s| s.len()).sum();

        if q.len() >= MAX_LINES || total_bytes + line.len() > MAX_BUFFER_BYTES {
            self.bytes_dropped.fetch_add(len, Ordering::Relaxed);
            // Drop oldest
            if let Some(old) = q.pop_front() {
                // count dropped
            }
        }
        q.push_back(line);
    }

    pub fn tail(&self, n: usize) -> Vec<String> {
        let q = self.lines.lock().unwrap();
        let start = if q.len() > n { q.len() - n } else { 0 };
        q.iter().skip(start).cloned().collect()
    }

    pub fn all(&self) -> Vec<String> {
        let q = self.lines.lock().unwrap();
        q.iter().cloned().collect()
    }
}

#[derive(Debug)]
pub struct Job {
    pub id: String,
    pub command: String,
    pub workdir: Option<String>,
    pub status: Arc<RwLock<JobStatus>>,
    pub pid: Arc<Mutex<Option<u32>>>,
    pub exit_code: Arc<Mutex<Option<i32>>>,
    pub stdout: Arc<StreamBuffer>,
    pub stderr: Arc<StreamBuffer>,
    pub started_at: i64,
    pub finished_at: Arc<Mutex<Option<i64>>>,
    #[cfg(unix)]
    pub child_handle: Arc<Mutex<Option<tokio::process::Child>>>,
    #[cfg(not(unix))]
    pub child_handle: Arc<Mutex<Option<tokio::process::Child>>>,
    pub done_notify: Arc<Notify>,
}

impl Job {
    pub fn new(command: String, workdir: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            id: Uuid::new_v4().to_string(),
            command,
            workdir,
            status: Arc::new(RwLock::new(JobStatus::Running)),
            pid: Arc::new(Mutex::new(None)),
            exit_code: Arc::new(Mutex::new(None)),
            stdout: StreamBuffer::new(),
            stderr: StreamBuffer::new(),
            started_at: Utc::now().timestamp(),
            finished_at: Arc::new(Mutex::new(None)),
            child_handle: Arc::new(Mutex::new(None)),
            done_notify: Arc::new(Notify::new()),
        })
    }

    pub fn is_done(&self) -> bool {
        let s = self.status.read().unwrap();
        matches!(*s, JobStatus::Finished | JobStatus::Failed | JobStatus::Killed)
    }

    pub fn get_status(&self) -> JobStatus {
        self.status.read().unwrap().clone()
    }
}

pub type JobStore = Arc<dashmap::DashMap<String, Arc<Job>>>;

pub fn new_store() -> JobStore {
    let store: JobStore = Arc::new(dashmap::DashMap::new());
    start_eviction_reaper(store.clone());
    store
}

fn start_eviction_reaper(store: JobStore) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(300)); // 5 min
        loop {
            ticker.tick().await;
            let now = Utc::now().timestamp();
            store.retain(|_, job| {
                if job.is_done() {
                    let finished = job.finished_at.lock().unwrap().unwrap_or(job.started_at);
                    // Keep for 2 hours
                    now - finished < 7200
                } else {
                    true
                }
            });
        }
    });
}

pub async fn start_job(store: &JobStore, command: String, workdir: Option<String>) -> Result<Arc<Job>> {
    let job = Job::new(command.clone(), workdir.clone());
    let job_clone = job.clone();
    store.insert(job.id.clone(), job.clone());

    tokio::spawn(async move {
        run_job(job_clone, command, workdir).await;
    });

    Ok(job)
}

async fn run_job(job: Arc<Job>, command: String, workdir: Option<String>) {
    use tokio::process::Command;

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", &command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &command]);
        c
    };

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    if let Some(wd) = &workdir {
        cmd.current_dir(wd);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to spawn job {}: {}", job.id, e);
            *job.status.write().unwrap() = JobStatus::Failed;
            *job.finished_at.lock().unwrap() = Some(Utc::now().timestamp());
            job.done_notify.notify_waiters();
            return;
        }
    };

    // Record PID
    if let Some(pid) = child.id() {
        *job.pid.lock().unwrap() = Some(pid);
    }

    let stdout_handle = child.stdout.take().map(|stdout| {
        let buf = job.stdout.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                buf.push(line);
            }
        })
    });

    let stderr_handle = child.stderr.take().map(|stderr| {
        let buf = job.stderr.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                buf.push(line);
            }
        })
    });

    // Store child for potential kill
    *job.child_handle.lock().unwrap() = Some(child);

    // Wait for output readers to finish
    if let Some(h) = stdout_handle {
        let _ = h.await;
    }
    if let Some(h) = stderr_handle {
        let _ = h.await;
    }

    // Wait for child to finish - take child out of mutex before awaiting
    let mut maybe_child = {
        let mut guard = job.child_handle.lock().unwrap();
        guard.take()
    };
    let exit_status = if let Some(ref mut child) = maybe_child {
        child.wait().await.ok()
    } else {
        None
    };

    let exit_code = exit_status.and_then(|s| s.code()).unwrap_or(-1);
    *job.exit_code.lock().unwrap() = Some(exit_code);
    *job.finished_at.lock().unwrap() = Some(Utc::now().timestamp());

    let final_status = if exit_code == 0 {
        JobStatus::Finished
    } else {
        JobStatus::Failed
    };
    *job.status.write().unwrap() = final_status;
    job.done_notify.notify_waiters();
}

pub fn evict_old_jobs(store: &JobStore, max_age_secs: i64) {
    let now = Utc::now().timestamp();
    store.retain(|_, job| {
        if job.is_done() {
            let finished = job.finished_at.lock().unwrap().unwrap_or(job.started_at);
            now - finished < max_age_secs
        } else {
            true
        }
    });
}

pub async fn kill_job(job: &Job) -> Result<()> {
    // Set status to killed
    *job.status.write().unwrap() = JobStatus::Killed;

    #[cfg(unix)]
    {
        if let Some(pid) = *job.pid.lock().unwrap() {
            use nix::sys::signal::{self, Signal};
            use nix::unistd::Pid;

            let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM);

            // Wait 5s then SIGKILL
            let pid_copy = pid;
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                let _ = signal::kill(Pid::from_raw(pid_copy as i32), Signal::SIGKILL);
            });
        }
    }

    #[cfg(not(unix))]
    {
        // Take the child out of the mutex before awaiting to keep the future Send
        let mut maybe_child = {
            let mut guard = job.child_handle.lock().unwrap();
            guard.take()
        };
        if let Some(ref mut child) = maybe_child {
            let _ = child.kill().await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn new_store_is_empty() {
        let store = Arc::new(dashmap::DashMap::<String, Arc<Job>>::new());
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn start_job_appears_in_store() {
        let store = Arc::new(dashmap::DashMap::new());
        let job = start_job(&store, "echo hello".into(), None).await.unwrap();
        assert!(store.contains_key(&job.id));
    }

    #[tokio::test]
    async fn job_completes_with_exit_code_zero() {
        let store = Arc::new(dashmap::DashMap::new());
        let cmd = if cfg!(windows) { "cmd /C exit 0".to_string() } else { "true".to_string() };
        let job = start_job(&store, cmd, None).await.unwrap();
        // Wait for completion
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if job.is_done() { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("job did not complete in time");
        assert_eq!(job.get_status(), JobStatus::Finished);
        assert_eq!(*job.exit_code.lock().unwrap(), Some(0));
    }

    #[tokio::test]
    async fn job_captures_stdout() {
        let store = Arc::new(dashmap::DashMap::new());
        let cmd = if cfg!(windows) {
            "cmd /C echo hello_output".to_string()
        } else {
            "echo hello_output".to_string()
        };
        let job = start_job(&store, cmd, None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if job.is_done() { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("job did not complete");
        let lines = job.stdout.all();
        let output = lines.join("\n");
        assert!(output.contains("hello_output"), "stdout: {:?}", output);
    }

    #[tokio::test]
    async fn job_fails_on_nonzero_exit() {
        let store = Arc::new(dashmap::DashMap::new());
        let cmd = if cfg!(windows) { "cmd /C exit 1".to_string() } else { "false".to_string() };
        let job = start_job(&store, cmd, None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if job.is_done() { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("job did not complete");
        assert_eq!(job.get_status(), JobStatus::Failed);
        assert_ne!(*job.exit_code.lock().unwrap(), Some(0));
    }

    #[tokio::test]
    async fn stream_buffer_push_and_tail() {
        let buf = StreamBuffer::new();
        buf.push("line1".into());
        buf.push("line2".into());
        buf.push("line3".into());
        let tail = buf.tail(2);
        assert_eq!(tail, vec!["line2", "line3"]);
    }

    #[tokio::test]
    async fn stream_buffer_all() {
        let buf = StreamBuffer::new();
        buf.push("a".into());
        buf.push("b".into());
        assert_eq!(buf.all(), vec!["a", "b"]);
    }

    #[tokio::test]
    async fn stream_buffer_tail_larger_than_contents() {
        let buf = StreamBuffer::new();
        buf.push("only".into());
        let tail = buf.tail(100);
        assert_eq!(tail, vec!["only"]);
    }

    #[tokio::test]
    async fn stream_buffer_bytes_total_tracked() {
        let buf = StreamBuffer::new();
        buf.push("hello".into()); // 5 bytes
        buf.push("world".into()); // 5 bytes
        assert_eq!(buf.bytes_total.load(std::sync::atomic::Ordering::Relaxed), 10);
    }

    #[test]
    fn evict_old_jobs_removes_finished() {
        // Use a runtime to create the store and jobs synchronously
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store: JobStore = Arc::new(dashmap::DashMap::new());
            let job = Job::new("echo".into(), None);
            // Mark as finished far in the past
            *job.status.write().unwrap() = JobStatus::Finished;
            *job.finished_at.lock().unwrap() = Some(Utc::now().timestamp() - 10000);
            store.insert(job.id.clone(), job);
            assert_eq!(store.len(), 1);
            evict_old_jobs(&store, 7200); // 2 hour max age
            assert_eq!(store.len(), 0);
        });
    }

    #[test]
    fn evict_old_jobs_keeps_running() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store: JobStore = Arc::new(dashmap::DashMap::new());
            let job = Job::new("sleep 1000".into(), None);
            // Status stays Running
            store.insert(job.id.clone(), job);
            evict_old_jobs(&store, 0); // max_age=0 should still keep running jobs
            assert_eq!(store.len(), 1);
        });
    }

    #[test]
    fn evict_old_jobs_keeps_recent_finished() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store: JobStore = Arc::new(dashmap::DashMap::new());
            let job = Job::new("echo".into(), None);
            *job.status.write().unwrap() = JobStatus::Finished;
            *job.finished_at.lock().unwrap() = Some(Utc::now().timestamp() - 60); // 1 min ago
            store.insert(job.id.clone(), job);
            evict_old_jobs(&store, 7200); // keep for 2h
            assert_eq!(store.len(), 1);
        });
    }
}
