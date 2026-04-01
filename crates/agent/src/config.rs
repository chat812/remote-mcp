use anyhow::{bail, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

// ── On-disk config format ─────────────────────────────────────────────────────

/// Full configuration stored in `agent.json`.
/// All fields have defaults so a partial file is valid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    pub port: u16,
    pub bind: String,
    pub token: String,
    pub log_level: String,
    pub max_concurrent_execs: usize,
    pub max_jobs: usize,
    #[serde(default)]
    pub allowed_ips: Vec<String>,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            port: 8765,
            bind: "0.0.0.0".into(),
            token: String::new(),
            log_level: "info".into(),
            max_concurrent_execs: 32,
            max_jobs: 100,
            allowed_ips: vec![],
        }
    }
}

impl FileConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }
}

/// Default config file path: `agent.json` next to the running binary.
pub fn default_config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("agent.json")))
        .unwrap_or_else(|| PathBuf::from("agent.json"))
}

// ── CLI args ──────────────────────────────────────────────────────────────────

#[derive(Parser, Debug, Clone)]
#[command(name = "agent", version, about = "Remote Exec Agent")]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Listen address (overrides config file)
    #[arg(long, default_value = "0.0.0.0")]
    pub bind: String,

    /// Listen port (overrides config file)
    #[arg(long, short)]
    pub port: Option<u16>,

    /// HMAC token (overrides config file; or set AGENT_TOKEN env var)
    #[arg(long, env = "AGENT_TOKEN")]
    pub token: Option<String>,

    /// Config file path [default: agent.json next to binary]
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Log level (overrides config file)
    #[arg(long)]
    pub log_level: Option<String>,

    /// Write logs to this file instead of stdout (useful for Windows service mode)
    #[arg(long)]
    pub log_file: Option<PathBuf>,

    /// Run as a Windows service (passed automatically by the install script)
    #[cfg(windows)]
    #[arg(long)]
    pub service: bool,

    /// Windows service name to register under
    #[cfg(windows)]
    #[arg(long, default_value = "RemoteExecAgent")]
    pub service_name: String,

    /// Max concurrent exec operations (overrides config file)
    #[arg(long)]
    pub max_concurrent_execs: Option<usize>,

    /// Max concurrent jobs (overrides config file)
    #[arg(long)]
    pub max_jobs: Option<usize>,
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum Command {
    /// Generate a config file and print the machine_add command for Claude
    Init {
        /// Agent listen port
        #[arg(long, default_value = "8765")]
        port: u16,

        /// Machine label shown in Claude (defaults to hostname)
        #[arg(long)]
        label: Option<String>,
    },
}

// ── Hot-reloadable subset ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotConfig {
    pub max_concurrent_execs: usize,
    pub max_jobs: usize,
    pub allowed_ips: Vec<String>,
    pub log_level: String,
}

impl Default for HotConfig {
    fn default() -> Self {
        Self {
            max_concurrent_execs: 32,
            max_jobs: 100,
            allowed_ips: vec![],
            log_level: "info".to_string(),
        }
    }
}

// ── Runtime config ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Config {
    pub bind: String,
    pub port: u16,
    pub token: String,
    pub hot: Arc<RwLock<HotConfig>>,
    pub config_path: Option<PathBuf>,
}

impl Config {
    /// Build from CLI args, transparently loading `agent.json` if no token is
    /// provided on the command line.
    pub fn resolve(args: &CliArgs) -> Result<Self> {
        // 1. Determine config file path
        let config_path = args.config.clone().unwrap_or_else(default_config_path);

        // 2. Try to load the file config (optional — ignored if absent)
        let file: FileConfig = if config_path.exists() {
            FileConfig::load(&config_path)?
        } else {
            FileConfig::default()
        };

        // 3. CLI overrides file; file overrides hard defaults
        let token = args
            .token
            .clone()
            .unwrap_or_else(|| file.token.clone());

        if token.is_empty() {
            bail!(
                "No token found. Run `agent init` to generate a config file, \
                 or pass --token / set AGENT_TOKEN."
            );
        }

        let port = args.port.unwrap_or(file.port);
        let bind = if args.bind != "0.0.0.0" {
            // explicit non-default CLI value
            args.bind.clone()
        } else {
            file.bind.clone()
        };

        let hot = HotConfig {
            max_concurrent_execs: args.max_concurrent_execs.unwrap_or(file.max_concurrent_execs),
            max_jobs: args.max_jobs.unwrap_or(file.max_jobs),
            allowed_ips: file.allowed_ips.clone(),
            log_level: args.log_level.clone().unwrap_or(file.log_level),
        };

        Ok(Self {
            bind,
            port,
            token,
            hot: Arc::new(RwLock::new(hot)),
            config_path: Some(config_path),
        })
    }

    pub fn listen_addr(&self) -> SocketAddr {
        format!("{}:{}", self.bind, self.port)
            .parse()
            .expect("invalid listen addr")
    }

    pub fn get_hot(&self) -> HotConfig {
        self.hot.read().unwrap().clone()
    }

    pub fn reload(&self) -> Result<()> {
        if let Some(path) = &self.config_path {
            let content = std::fs::read_to_string(path)?;
            let new_config: HotConfig = serde_json::from_str(&content)?;
            *self.hot.write().unwrap() = new_config;
            info!("Config reloaded from {}", path.display());
        }
        Ok(())
    }

    /// Convenience: log level from the hot config.
    pub fn log_level(&self) -> String {
        self.hot.read().unwrap().log_level.clone()
    }
}

// ── `agent init` implementation ───────────────────────────────────────────────

/// Detect the machine's outbound LAN IP without making an actual connection.
fn local_ip() -> String {
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

/// Generate a cryptographically random 64-character hex token using two UUIDs.
fn generate_token() -> String {
    use uuid::Uuid;
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Run `agent init`: write config file and print the `machine_add` command.
pub fn run_init(port: u16, label: Option<String>) -> Result<()> {
    let config_path = default_config_path();

    if config_path.exists() {
        // Re-read to show the existing token rather than regenerating
        let existing = FileConfig::load(&config_path)?;
        print_init_output(&config_path, &existing, port, label.as_deref());
        eprintln!("(Config already exists at {} — not overwritten.)", config_path.display());
        return Ok(());
    }

    let token = generate_token();
    let hostname = hostname_or_default();

    let file_config = FileConfig {
        port,
        bind: "0.0.0.0".into(),
        token: token.clone(),
        log_level: "info".into(),
        max_concurrent_execs: 32,
        max_jobs: 100,
        allowed_ips: vec![],
    };

    // Write config
    if let Some(parent) = config_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let json = serde_json::to_string_pretty(&file_config)?;
    std::fs::write(&config_path, &json)?;

    let label = label.as_deref().unwrap_or(&hostname);
    print_init_output(&config_path, &file_config, port, Some(label));
    Ok(())
}

fn print_init_output(config_path: &Path, cfg: &FileConfig, _port: u16, label: Option<&str>) {
    let ip = local_ip();
    let hostname = hostname_or_default();
    let label = label.unwrap_or(&hostname);

    println!();
    println!("Config written to: {}", config_path.display());
    println!();
    println!("Add this machine in Claude:");
    println!();
    println!(
        "  machine_add label=\"{}\" host=\"{}\" port={} transport=\"agent\" \
         agent_url=\"http://{}:{}\" agent_token=\"{}\"",
        label, ip, cfg.port, ip, cfg.port, cfg.token
    );
    println!();
    println!("Start the agent:");
    println!();
    println!("  ./agent");
    println!();
}

fn hostname_or_default() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "my-machine".into())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp_config(cfg: &FileConfig) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        let json = serde_json::to_string(cfg).unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f
    }

    #[test]
    fn file_config_round_trip() {
        let cfg = FileConfig {
            port: 9000,
            bind: "127.0.0.1".into(),
            token: "tok123".into(),
            log_level: "debug".into(),
            max_concurrent_execs: 8,
            max_jobs: 50,
            allowed_ips: vec!["10.0.0.1".into()],
        };
        let f = write_temp_config(&cfg);
        let loaded = FileConfig::load(f.path()).unwrap();
        assert_eq!(loaded.port, 9000);
        assert_eq!(loaded.token, "tok123");
        assert_eq!(loaded.bind, "127.0.0.1");
        assert_eq!(loaded.max_concurrent_execs, 8);
        assert_eq!(loaded.allowed_ips, vec!["10.0.0.1"]);
    }

    #[test]
    fn generate_token_is_64_chars() {
        let t = generate_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn local_ip_returns_something() {
        let ip = local_ip();
        assert!(!ip.is_empty());
        // Should be a valid IP
        assert!(ip.parse::<std::net::IpAddr>().is_ok(), "invalid IP: {}", ip);
    }
}
