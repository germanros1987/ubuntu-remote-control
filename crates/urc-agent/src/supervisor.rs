//! In-process supervisor: wait for session, run backends, health-check, restart on failure.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio::time::{interval, Duration};
use tracing::{info, warn};

use crate::backend::BackendManager;
use crate::coordinator::CoordinatorClient;
use crate::health::tcp_open;
use crate::health::{AgentStatus, STATUS_PATH};
use crate::session::SessionDetector;
use crate::tunnel::TlsTunnel;
use urc_common::AgentConfig;

const SESSION_POLL_SECS: u64 = 30;
const HEALTH_TICK_SECS: u64 = 30;
const RECOVERY_PAUSE_SECS: u64 = 5;

struct RunningStack {
    backend: BackendManager,
    vnc_port: u16,
    coordinator: Arc<CoordinatorClient>,
    coord_handle: tokio::task::JoinHandle<()>,
    files_handle: Option<tokio::task::JoinHandle<()>>,
    tls_handle: Option<tokio::task::JoinHandle<()>>,
}

pub async fn run_supervisor(config: AgentConfig) -> Result<()> {
    let mut cycle: u64 = 0;
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });

    loop {
        if *shutdown_rx.borrow() {
            info!("shutdown requested");
            break;
        }

        cycle = cycle.saturating_add(1);
        info!(cycle, "supervisor cycle start");

        match run_one_cycle(&config, cycle, &mut shutdown_rx).await {
            Ok(()) => info!(cycle, "supervisor cycle ended normally"),
            Err(e) => warn!(cycle, error = %e, "supervisor cycle failed"),
        }

        if *shutdown_rx.borrow() {
            break;
        }

        tokio::time::sleep(Duration::from_secs(RECOVERY_PAUSE_SECS)).await;
    }

    Ok(())
}

async fn run_one_cycle(
    config: &AgentConfig,
    cycle: u64,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> Result<()> {
    let session = wait_for_session(config, shutdown_rx).await?;
    info!(
        backend = ?session.backend_kind,
        display = ?session.display,
        "session ready"
    );

    let backend_mgr = BackendManager::new(config, &session)?;
    backend_mgr.start().await?;
    let vnc_port = 5900u16;

    let files_handle = if let Some(root) = &config.files_root {
        Some(
            urc_files::spawn_files_server(
                PathBuf::from(root),
                "127.0.0.1",
                urc_common::DEFAULT_FILES_PORT,
            )
            .await?,
        )
    } else {
        None
    };

    let coordinator = Arc::new(CoordinatorClient::new(config.clone(), vnc_port));
    let coord_client = coordinator.clone();
    let coord_handle = tokio::spawn(async move {
        coord_client.run_loop().await;
    });

    let tls_handle = if !config.insecure {
        let tunnel = TlsTunnel::new(config, vnc_port)?;
        Some(tokio::spawn(async move {
            if let Err(e) = tunnel.serve().await {
                warn!(error = %e, "TLS tunnel stopped");
            }
        }))
    } else {
        None
    };

    let stack = RunningStack {
        backend: backend_mgr,
        vnc_port,
        coordinator,
        coord_handle,
        files_handle,
        tls_handle,
    };

    info!(vnc_port, "stack running — entering health loop");
    health_loop(config, &stack, cycle, shutdown_rx).await?;

    teardown(stack).await;
    Ok(())
}

async fn wait_for_session(
    config: &AgentConfig,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> Result<crate::session::SessionInfo> {
    let mut poll = interval(Duration::from_secs(SESSION_POLL_SECS));
    poll.tick().await;

    loop {
        if *shutdown_rx.borrow() {
            anyhow::bail!("shutdown while waiting for session");
        }

        match SessionDetector::detect(config.backend) {
            Ok(info) => return Ok(info),
            Err(e) => {
                warn!(error = %e, "no graphical session yet — retrying");
                write_waiting_status(&e.to_string());
            }
        }

        tokio::select! {
            _ = poll.tick() => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    anyhow::bail!("shutdown while waiting for session");
                }
            }
        }
    }
}

async fn health_loop(
    config: &AgentConfig,
    stack: &RunningStack,
    cycle: u64,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> Result<()> {
    let mut tick = interval(Duration::from_secs(HEALTH_TICK_SECS));
    tick.tick().await;

    loop {
        let vnc_ok = stack.backend.health_check().await;
        let coord_ok = stack.coordinator.is_connected();
        let files_ok = tcp_open(urc_common::DEFAULT_FILES_PORT).await;

        let healthy = vnc_ok && coord_ok;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let status = AgentStatus {
            healthy,
            session_detected: true,
            vnc_port_open: vnc_ok,
            coordinator_connected: coord_ok,
            files_port_open: files_ok,
            backend: Some(format!("{:?}", config.backend)),
            display: None,
            last_error: if healthy {
                None
            } else {
                Some(format!(
                    "vnc={} coordinator={}",
                    if vnc_ok { "ok" } else { "down" },
                    if coord_ok { "ok" } else { "down" }
                ))
            },
            updated_at_unix: now,
            supervisor_cycle: cycle,
        };
        let _ = status.write(std::path::Path::new(STATUS_PATH));

        if !healthy {
            warn!(
                vnc_ok,
                coord_ok,
                "health check failed — restarting stack"
            );
            return Ok(());
        }

        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

async fn teardown(mut stack: RunningStack) {
    stack.coord_handle.abort();
    if let Some(h) = stack.files_handle.take() {
        h.abort();
    }
    if let Some(h) = stack.tls_handle.take() {
        h.abort();
    }
    stack.backend.stop().await;

    stack.coordinator.abort_all_tunnels().await;
}

fn write_waiting_status(err: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let status = AgentStatus {
        healthy: false,
        session_detected: false,
        vnc_port_open: false,
        coordinator_connected: false,
        files_port_open: false,
        backend: None,
        display: None,
        last_error: Some(err.to_string()),
        updated_at_unix: now,
        supervisor_cycle: 0,
    };
    let _ = status.write(std::path::Path::new(STATUS_PATH));
}
