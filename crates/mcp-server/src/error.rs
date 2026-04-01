use thiserror::Error;

#[derive(Debug, Error)]
pub enum RemoteExecError {
    #[error("Machine '{0}' not found. Run machine_list to see registered machines.")]
    MachineNotFound(String),
    #[error("Connection to '{machine}' failed: {reason}")]
    ConnectionFailed { machine: String, reason: String },
    #[error("Circuit open for '{machine}' — too many recent failures. Retry after {retry_after_secs}s.")]
    CircuitOpen { machine: String, retry_after_secs: u64 },
    #[error("Command timed out on '{machine}' after {after_secs}s.")]
    Timeout { machine: String, after_secs: u64 },
    #[error("Tool '{tool}' requires agent transport. Set transport=agent or agent+ssh for this machine.")]
    AgentRequired { tool: String },
    #[error("Output truncated — showing {shown_bytes}KB of {total_bytes}KB. Use log_tail with cursor or job_logs with tail=N to paginate.")]
    OutputTruncated { shown_bytes: usize, total_bytes: usize },
    #[error("Machine '{machine}' unreachable (last seen {last_seen_secs}s ago). Run machine_test to check.")]
    MachineUnreachable { machine: String, last_seen_secs: u64 },
    #[error("SSH error on '{machine}': {reason}")]
    SshError { machine: String, reason: String },
    #[error("Agent error on '{machine}': HTTP {status} — {body}")]
    AgentHttpError { machine: String, status: u16, body: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_not_found_message() {
        let e = RemoteExecError::MachineNotFound("prod-1".into());
        assert_eq!(e.to_string(), "Machine 'prod-1' not found. Run machine_list to see registered machines.");
    }

    #[test]
    fn connection_failed_message() {
        let e = RemoteExecError::ConnectionFailed {
            machine: "web-01".into(),
            reason: "refused".into(),
        };
        assert_eq!(e.to_string(), "Connection to 'web-01' failed: refused");
    }

    #[test]
    fn circuit_open_message() {
        let e = RemoteExecError::CircuitOpen {
            machine: "db-01".into(),
            retry_after_secs: 25,
        };
        let s = e.to_string();
        assert!(s.contains("db-01"));
        assert!(s.contains("25s"));
    }

    #[test]
    fn timeout_message() {
        let e = RemoteExecError::Timeout {
            machine: "slow-box".into(),
            after_secs: 120,
        };
        assert_eq!(e.to_string(), "Command timed out on 'slow-box' after 120s.");
    }

    #[test]
    fn agent_required_message() {
        let e = RemoteExecError::AgentRequired { tool: "docker_ps".into() };
        let s = e.to_string();
        assert!(s.contains("docker_ps"));
        assert!(s.contains("agent"));
    }

    #[test]
    fn output_truncated_message() {
        let e = RemoteExecError::OutputTruncated { shown_bytes: 100, total_bytes: 500 };
        let s = e.to_string();
        assert!(s.contains("100KB"));
        assert!(s.contains("500KB"));
    }

    #[test]
    fn machine_unreachable_message() {
        let e = RemoteExecError::MachineUnreachable {
            machine: "offline-box".into(),
            last_seen_secs: 300,
        };
        let s = e.to_string();
        assert!(s.contains("offline-box"));
        assert!(s.contains("300s"));
    }

    #[test]
    fn ssh_error_message() {
        let e = RemoteExecError::SshError {
            machine: "host-1".into(),
            reason: "auth failed".into(),
        };
        assert_eq!(e.to_string(), "SSH error on 'host-1': auth failed");
    }

    #[test]
    fn agent_http_error_message() {
        let e = RemoteExecError::AgentHttpError {
            machine: "app-1".into(),
            status: 500,
            body: "internal error".into(),
        };
        let s = e.to_string();
        assert!(s.contains("app-1"));
        assert!(s.contains("500"));
        assert!(s.contains("internal error"));
    }
}
