use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub hostname: String,
    pub has_systemd: bool,
    pub has_docker: bool,
    pub has_git: bool,
    pub has_ui_automation: bool,
}

pub fn detect() -> Capabilities {
    Capabilities {
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        hostname: hostname(),
        has_systemd: has_systemd(),
        has_docker: which::which("docker").is_ok(),
        has_git: which::which("git").is_ok(),
        has_ui_automation: cfg!(windows),
    }
}

fn hostname() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string())
    }
    #[cfg(not(windows))]
    {
        let from_file = std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        from_file
            .or_else(|| std::env::var("HOSTNAME").ok())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

fn has_systemd() -> bool {
    #[cfg(windows)]
    { false }
    #[cfg(not(windows))]
    { which::which("systemctl").is_ok() }
}
