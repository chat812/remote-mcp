use serde::Serialize;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug)]
pub struct Metrics {
    pub started_at: Instant,
    pub execs_total: AtomicU64,
    pub exec_errors_total: AtomicU64,
    pub jobs_started_total: AtomicU64,
    pub jobs_running: AtomicI64,
    pub sessions_open: AtomicI64,
    pub bytes_uploaded_total: AtomicU64,
    pub bytes_downloaded_total: AtomicU64,
    pub requests_rejected_429: AtomicU64,
    pub auth_failures_total: AtomicU64,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            started_at: Instant::now(),
            execs_total: AtomicU64::new(0),
            exec_errors_total: AtomicU64::new(0),
            jobs_started_total: AtomicU64::new(0),
            jobs_running: AtomicI64::new(0),
            sessions_open: AtomicI64::new(0),
            bytes_uploaded_total: AtomicU64::new(0),
            bytes_downloaded_total: AtomicU64::new(0),
            requests_rejected_429: AtomicU64::new(0),
            auth_failures_total: AtomicU64::new(0),
        })
    }

    pub fn inc_execs(&self) {
        self.execs_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_exec_errors(&self) {
        self.exec_errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_jobs_started(&self) {
        self.jobs_started_total.fetch_add(1, Ordering::Relaxed);
        self.jobs_running.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_jobs_running(&self) {
        self.jobs_running.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn inc_sessions(&self) {
        self.sessions_open.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_sessions(&self) {
        self.sessions_open.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn add_bytes_uploaded(&self, n: u64) {
        self.bytes_uploaded_total.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_bytes_downloaded(&self, n: u64) {
        self.bytes_downloaded_total.fetch_add(n, Ordering::Relaxed);
    }

    pub fn inc_rejected(&self) {
        self.requests_rejected_429.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_auth_failures(&self) {
        self.auth_failures_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            uptime_secs: self.started_at.elapsed().as_secs(),
            execs_total: self.execs_total.load(Ordering::Relaxed),
            exec_errors_total: self.exec_errors_total.load(Ordering::Relaxed),
            jobs_started_total: self.jobs_started_total.load(Ordering::Relaxed),
            jobs_running: self.jobs_running.load(Ordering::Relaxed),
            sessions_open: self.sessions_open.load(Ordering::Relaxed),
            bytes_uploaded_total: self.bytes_uploaded_total.load(Ordering::Relaxed),
            bytes_downloaded_total: self.bytes_downloaded_total.load(Ordering::Relaxed),
            requests_rejected_429: self.requests_rejected_429.load(Ordering::Relaxed),
            auth_failures_total: self.auth_failures_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct MetricsSnapshot {
    pub uptime_secs: u64,
    pub execs_total: u64,
    pub exec_errors_total: u64,
    pub jobs_started_total: u64,
    pub jobs_running: i64,
    pub sessions_open: i64,
    pub bytes_uploaded_total: u64,
    pub bytes_downloaded_total: u64,
    pub requests_rejected_429: u64,
    pub auth_failures_total: u64,
}
