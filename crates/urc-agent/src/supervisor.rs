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
const RECOVERY_PAUSE_MAX_SECS: u64 = 300;

struct RunningStack {
    backend: BackendManager,
    vnc_port: u16,
    coordinator: Arc<CoordinatorClient>,
    coord_handle: tokio::task::JoinHandle<()>,
    web_handle: Option<tokio::task::JoinHandle<()>>,
    vnc_tls_handle: Option<tokio::task::JoinHandle<()>>,
    web_tls_handle: Option<tokio::task::JoinHandle<()>>,
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

        // Back off after failures so we do not exhaust X client slots (MaxClients ~256).
        let pause = RECOVERY_PAUSE_SECS.saturating_mul(1u64 << cycle.min(6).saturating_sub(1));
        let pause = pause.min(RECOVERY_PAUSE_MAX_SECS);
        if pause > RECOVERY_PAUSE_SECS {
            warn!(pause_secs = pause, "backing off before next supervisor cycle");
        }
        tokio::time::sleep(Duration::from_secs(pause)).await;
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

    // Unified web app: serves SPA, WebSocket-to-VNC bridge, files API, clipboard helper.
    let web_handle = if let Some(root) = &config.files_root {
        let bind: std::net::SocketAddr =
            ([127, 0, 0, 1], urc_common::DEFAULT_WEB_INTERNAL_PORT).into();
        let desktop = urc_web::DesktopSession {
            username: session.username.clone(),
            display: session.display.clone().unwrap_or_else(|| ":0".to_string()),
            xauthority: session.xauthority.clone(),
        };
        Some(
            urc_web::spawn_web_server(PathBuf::from(root), bind, vnc_port, desktop).await?,
        )
    } else {
        None
    };

    let use_coordinator = !config.coordinator_url.trim().is_empty();
    let coordinator = Arc::new(CoordinatorClient::new(config.clone(), vnc_port));
    let coord_handle = if use_coordinator {
        let coord_client = coordinator.clone();
        tokio::spawn(async move {
            coord_client.run_loop().await;
        })
    } else {
        info!(
            vnc_tls = config.listen_tls_port,
            web_tls = config.listen_web_tls_port,
            "coordinator off — reachable via Tailscale TLS"
        );
        tokio::spawn(async { std::future::pending::<()>().await })
    };

    let (vnc_tls_handle, web_tls_handle) = if !config.insecure {
        let vnc_tunnel =
            TlsTunnel::new(config, config.listen_tls_port, vnc_port, "vnc")?;
        let web_tunnel = TlsTunnel::new(
            config,
            config.listen_web_tls_port,
            urc_common::DEFAULT_WEB_INTERNAL_PORT,
            "web",
        )?;
        let v = tokio::spawn(async move {
            if let Err(e) = vnc_tunnel.serve().await {
                warn!(error = %e, "VNC TLS tunnel stopped");
            }
        });
        let w = tokio::spawn(async move {
            if let Err(e) = web_tunnel.serve().await {
                warn!(error = %e, "web TLS tunnel stopped");
            }
        });
        (Some(v), Some(w))
    } else {
        (None, None)
    };

    let stack = RunningStack {
        backend: backend_mgr,
        vnc_port,
        coordinator,
        coord_handle,
        web_handle,
        vnc_tls_handle,
        web_tls_handle,
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
        let needs_coordinator = !config.coordinator_url.trim().is_empty();
        let coord_ok = !needs_coordinator || stack.coordinator.is_connected();
        let files_ok = tcp_open(urc_common::DEFAULT_WEB_INTERNAL_PORT).await;

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
    if let Some(h) = stack.web_handle.take() {
        h.abort();
    }
    if let Some(h) = stack.vnc_tls_handle.take() {
        h.abort();
    }
    if let Some(h) = stack.web_tls_handle.take() {
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
