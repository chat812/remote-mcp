use agent::{capabilities, config, jobs, metrics, routes, sessions};
use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use config::{CliArgs, Command};

// ── Windows service plumbing ─────────────────────────────────────────────────

#[cfg(windows)]
windows_service::define_windows_service!(ffi_service_main, win_service_main);

#[cfg(windows)]
fn win_service_main(_svc_args: Vec<std::ffi::OsString>) {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use windows_service::{
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
    };

    let args = CliArgs::parse();
    let cfg = match config::Config::resolve(&args) {
        Ok(c) => c,
        Err(e) => {
            warn!("Config error: {e}");
            return;
        }
    };
    init_logging(&args, &cfg);

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let stop_tx = Arc::new(Mutex::new(Some(stop_tx)));

    let event_handler = {
        let stop_tx = stop_tx.clone();
        move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    if let Ok(mut g) = stop_tx.lock() {
                        if let Some(tx) = g.take() {
                            let _ = tx.send(());
                        }
                    }
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        }
    };

    let status_handle =
        match service_control_handler::register(&args.service_name, event_handler) {
            Ok(h) => h,
            Err(e) => {
                warn!("Failed to register service control handler: {e}");
                return;
            }
        };

    let running = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    if let Err(e) = status_handle.set_service_status(running) {
        warn!("Failed to set SERVICE_RUNNING: {e}");
        return;
    }

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let shutdown = async move { let _ = stop_rx.await; };
    if let Err(e) = rt.block_on(run_server(cfg, shutdown)) {
        warn!("Server exited with error: {e}");
    }

    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    });
}

// ── Logging init ─────────────────────────────────────────────────────────────

fn init_logging(args: &CliArgs, cfg: &config::Config) {
    let level = args.log_level.clone().unwrap_or_else(|| cfg.log_level());

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&level));

    if let Some(log_file) = &args.log_file {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
            .expect("failed to open log file");
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .try_init();
    }
}

// ── Core async server ────────────────────────────────────────────────────────

async fn run_server(
    config: config::Config,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    info!("Starting remote-exec agent v{}", env!("CARGO_PKG_VERSION"));

    info!("Detecting capabilities...");
    let caps = capabilities::detect();
    info!(
        os = %caps.os,
        arch = %caps.arch,
        hostname = %caps.hostname,
        has_systemd = caps.has_systemd,
        has_docker = caps.has_docker,
        has_git = caps.has_git,
        "Capabilities detected"
    );
    let caps = Arc::new(caps);

    let job_store = jobs::new_store();
    let session_store = sessions::new_store();
    let metrics = metrics::Metrics::new();

    let hot = config.get_hot();
    let exec_semaphore = Arc::new(Semaphore::new(hot.max_concurrent_execs));
    let file_semaphore = Arc::new(Semaphore::new(8));

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let config_clone = config.clone();
        tokio::spawn(async move {
            let mut sighup =
                signal(SignalKind::hangup()).expect("failed to register SIGHUP handler");
            loop {
                sighup.recv().await;
                info!("Received SIGHUP — reloading config");
                if let Err(e) = config_clone.reload() {
                    warn!("Config reload failed: {}", e);
                }
            }
        });
    }

    let app_state = routes::AppState {
        config: config.clone(),
        jobs: job_store.clone(),
        sessions: session_store.clone(),
        metrics: metrics.clone(),
        capabilities: caps,
        exec_semaphore,
        file_semaphore,
    };
    let router = routes::build_router(app_state);

    let addr = config.listen_addr();
    info!(%addr, "Listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await?;

    info!("Agent stopped cleanly");
    Ok(())
}

// ── Entry point ──────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = CliArgs::parse();

    // Handle `agent init` before anything else — no config or logging needed.
    if let Some(Command::Init { port, label }) = &args.command {
        return config::run_init(*port, label.clone());
    }

    // Windows service mode: hand control to SCM.
    #[cfg(windows)]
    if args.service {
        use windows_service::service_dispatcher;
        service_dispatcher::start(&args.service_name, ffi_service_main)
            .map_err(|e| anyhow::anyhow!("service dispatcher failed: {e}"))?;
        return Ok(());
    }

    // Load config (file + CLI overrides).
    let cfg = config::Config::resolve(&args)?;
    init_logging(&args, &cfg);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_server(cfg, async {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sigterm =
                    signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = sigterm.recv() => {},
                }
            }
            #[cfg(not(unix))]
            {
                tokio::signal::ctrl_c().await.ok();
            }
            info!("Shutting down — draining requests (max 30s)");
        }))
}
