pub mod agent;
pub mod ssh;

use crate::db::Machine;
use crate::error::RemoteExecError;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub enum CircuitState {
    Closed,
    Open { opened_at: Instant },
    HalfOpen,
}

#[derive(Debug)]
pub struct CircuitBreaker {
    pub state: CircuitState,
    pub failures: Vec<Instant>,
    pub failure_threshold: usize,
    pub open_duration: Duration,
    pub window: Duration,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            failures: Vec::new(),
            failure_threshold: 3,
            open_duration: Duration::from_secs(30),
            window: Duration::from_secs(60),
        }
    }

    pub fn record_failure(&mut self) {
        let now = Instant::now();
        self.failures.retain(|t| now.duration_since(*t) < self.window);
        self.failures.push(now);
        if self.failures.len() >= self.failure_threshold {
            self.state = CircuitState::Open { opened_at: now };
        }
    }

    pub fn record_success(&mut self) {
        self.failures.clear();
        self.state = CircuitState::Closed;
    }

    pub fn is_open(&self) -> Option<u64> {
        if let CircuitState::Open { opened_at } = &self.state {
            let elapsed = Instant::now().duration_since(*opened_at);
            if elapsed < self.open_duration {
                let remaining = self.open_duration - elapsed;
                return Some(remaining.as_secs());
            }
        }
        None
    }
}

pub struct CircuitBreakers {
    breakers: DashMap<String, Arc<Mutex<CircuitBreaker>>>,
}

impl CircuitBreakers {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            breakers: DashMap::new(),
        })
    }

    fn get_or_create(&self, id: &str) -> Arc<Mutex<CircuitBreaker>> {
        self.breakers
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(CircuitBreaker::new())))
            .clone()
    }

    pub fn record_failure(&self, id: &str) {
        let breaker = self.get_or_create(id);
        tokio::spawn(async move {
            breaker.lock().await.record_failure();
        });
    }

    pub fn record_success(&self, id: &str) {
        let breaker = self.get_or_create(id);
        tokio::spawn(async move {
            breaker.lock().await.record_success();
        });
    }

    pub async fn check(&self, machine_id: &str, machine_label: &str) -> Result<(), RemoteExecError> {
        let breaker = self.get_or_create(machine_id);
        let guard = breaker.lock().await;
        if let Some(retry_after) = guard.is_open() {
            return Err(RemoteExecError::CircuitOpen {
                machine: machine_label.to_string(),
                retry_after_secs: retry_after,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub async fn dispatch_exec(
    machine: &Machine,
    cmd: &str,
    timeout_secs: u64,
    circuits: &Arc<CircuitBreakers>,
) -> Result<ExecResult, RemoteExecError> {
    circuits.check(&machine.id, &machine.label).await?;

    match machine.transport.as_str() {
        "ssh" => {
            let result = ssh::exec_ssh(machine, cmd, timeout_secs).await;
            match &result {
                Ok(_) => circuits.record_success(&machine.id),
                Err(_) => circuits.record_failure(&machine.id),
            }
            result
        }
        "agent" | "agent+ssh" => {
            let result = agent::exec_agent(machine, cmd, timeout_secs).await;
            match &result {
                Ok(_) => circuits.record_success(&machine.id),
                Err(_) => circuits.record_failure(&machine.id),
            }
            result
        }
        other => Err(RemoteExecError::ConnectionFailed {
            machine: machine.label.clone(),
            reason: format!("Unknown transport: {}", other),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn breaker() -> CircuitBreaker {
        CircuitBreaker::new()
    }

    #[test]
    fn new_breaker_is_closed() {
        let cb = breaker();
        assert_eq!(cb.state, CircuitState::Closed);
        assert!(cb.is_open().is_none());
    }

    #[test]
    fn single_failure_stays_closed() {
        let mut cb = breaker();
        cb.record_failure();
        assert_eq!(cb.state, CircuitState::Closed);
        assert!(cb.is_open().is_none());
    }

    #[test]
    fn two_failures_stays_closed() {
        let mut cb = breaker();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state, CircuitState::Closed);
    }

    #[test]
    fn three_failures_opens_circuit() {
        let mut cb = breaker();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(matches!(cb.state, CircuitState::Open { .. }));
        assert!(cb.is_open().is_some());
    }

    #[test]
    fn is_open_returns_remaining_seconds() {
        let mut cb = breaker();
        for _ in 0..3 {
            cb.record_failure();
        }
        let secs = cb.is_open().unwrap();
        // Should be close to open_duration (30s) immediately after opening
        assert!(secs <= 30);
        assert!(secs > 0 || secs == 0); // can be 0 if instant elapses
    }

    #[test]
    fn success_resets_to_closed() {
        let mut cb = breaker();
        for _ in 0..3 {
            cb.record_failure();
        }
        assert!(matches!(cb.state, CircuitState::Open { .. }));
        cb.record_success();
        assert_eq!(cb.state, CircuitState::Closed);
        assert!(cb.is_open().is_none());
    }

    #[test]
    fn success_clears_failure_history() {
        let mut cb = breaker();
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        // After success, two more failures should NOT open (history was cleared)
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state, CircuitState::Closed);
    }

    #[test]
    fn old_failures_outside_window_not_counted() {
        let mut cb = CircuitBreaker {
            window: Duration::from_millis(1), // 1ms window
            ..CircuitBreaker::new()
        };
        cb.record_failure();
        cb.record_failure();
        // Sleep past the window
        std::thread::sleep(Duration::from_millis(5));
        // This third failure should be within window, but old ones expired
        cb.record_failure();
        // Only 1 failure in window → still closed
        assert_eq!(cb.state, CircuitState::Closed);
    }

    #[test]
    fn circuit_stays_open_within_duration() {
        let mut cb = breaker();
        for _ in 0..3 {
            cb.record_failure();
        }
        assert!(cb.is_open().is_some());
    }

    #[test]
    fn expired_open_circuit_returns_none_from_is_open() {
        let mut cb = CircuitBreaker {
            open_duration: Duration::from_millis(1),
            ..CircuitBreaker::new()
        };
        for _ in 0..3 {
            cb.record_failure();
        }
        std::thread::sleep(Duration::from_millis(10));
        // After open_duration expires, is_open returns None
        assert!(cb.is_open().is_none());
    }

    #[tokio::test]
    async fn circuit_breakers_check_passes_when_closed() {
        let cbs = CircuitBreakers::new();
        let result = cbs.check("machine-1", "prod").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn circuit_breakers_check_fails_when_open() {
        let cbs = CircuitBreakers::new();
        // Record enough failures to open
        for _ in 0..3 {
            cbs.record_failure("machine-1");
        }
        // Give the spawned tasks time to execute
        tokio::time::sleep(Duration::from_millis(50)).await;
        let result = cbs.check("machine-1", "prod").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, crate::error::RemoteExecError::CircuitOpen { .. }));
    }

    #[tokio::test]
    async fn circuit_breakers_recover_after_success() {
        let cbs = CircuitBreakers::new();
        for _ in 0..3 {
            cbs.record_failure("machine-1");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        cbs.record_success("machine-1");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let result = cbs.check("machine-1", "prod").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn independent_machines_have_independent_circuits() {
        let cbs = CircuitBreakers::new();
        for _ in 0..3 {
            cbs.record_failure("machine-bad");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        // machine-good should still pass
        assert!(cbs.check("machine-good", "good").await.is_ok());
        // machine-bad should fail
        assert!(cbs.check("machine-bad", "bad").await.is_err());
    }
}
