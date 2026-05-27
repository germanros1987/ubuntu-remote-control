//! In-process supervisor.
//!
//! Serving is DECOUPLED from VNC. After the first detected session we bind the
//! persistent listeners ONCE — the urc-web server (16080), the coordinator loop,
//! and (unless `insecure`) the two TLS tunnels (vnc_tls 15900→5900,
//! web_tls 15901→16080) — and keep them up for the agent's lifetime. A VNC/display
//! failure must NEVER tear these down: the web UI binds fine even with VNC down
//! (noVNC shows "reconnecting" and the WS bridge reconnects to 5900 on demand).
//!
//! The VNC backend runs as an INDEPENDENT supervised child loop with its own
//! backoff. A separate status/liveness loop reports health based on the WEB/FILES
//! port being reachable (VNC state is reported but never fatal).

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
const STATUS_TICK_SECS: u64 = 30;
const VNC_HEALTH_TICK_SECS: u64 = 10;
const RECOVERY_PAUSE_SECS: u64 = 5;
const RECOVERY_PAUSE_MAX_SECS: u64 = 300;
/// Longer fixed cooldown after the X display reports "Maximum number of clients
/// reached": rebuilding the VNC server every few seconds is exactly what saturated
/// the display, so we wait for it to drain instead of churning.
const SATURATION_COOLDOWN_SECS: u64 = 60;
/// Backoff for re-serving a TLS tunnel whose `serve()` returned. Short so a
/// transient accept()/bind error self-heals quickly, capped so a persistent
/// failure (e.g. port held by another process) doesn't busy-loop.
const TUNNEL_RETRY_SECS: u64 = 2;
const TUNNEL_RETRY_MAX_SECS: u64 = 30;

const VNC_PORT: u16 = 5900;

/// Persistent serving stack. Bound once from the first detected session and kept
/// alive until shutdown. Does NOT include the VNC backend — that is supervised
/// independently so its failures cannot take the listeners down.
struct ServingStack {
    coordinator: Arc<CoordinatorClient>,
    coord_handle: tokio::task::JoinHandle<()>,
    web_handle: Option<tokio::task::JoinHandle<()>>,
    vnc_tls_handle: Option<tokio::task::JoinHandle<()>>,
    web_tls_handle: Option<tokio::task::JoinHandle<()>>,
}

pub async fn run_supervisor(config: AgentConfig) -> Result<()> {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });

    // Wait for the first graphical session. The listeners are bound from this
    // session's identity (username/display/xauthority for the urc-web clipboard).
    let session = wait_for_session(&config, &mut shutdown_rx).await?;
    if *shutdown_rx.borrow() {
        info!("shutdown requested before session ready");
        return Ok(());
    }
    info!(
        backend = ?session.backend_kind,
        display = ?session.display,
        "session ready — binding persistent listeners"
    );

    // 1) Bind the persistent serving stack ONCE. This never gets torn down for a
    //    VNC failure; only shutdown aborts it.
    let stack = spawn_serving_stack(&config, &session, &shutdown_rx).await?;

    // 2) Supervise the VNC backend independently. It restarts on its own backoff
    //    without ever touching the listeners above.
    let vnc_health = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let vnc_loop = tokio::spawn(vnc_supervisor_loop(
        config.clone(),
        session.clone(),
        vnc_health.clone(),
        shutdown_rx.clone(),
    ));

    // 3) Status/liveness loop. Liveness = web/files port reachable AND coordinator
    //    ok (if used). VNC state is reported but never fatal.
    status_loop(&config, &stack.coordinator, &vnc_health, &mut shutdown_rx).await;

    info!("shutdown requested — tearing down");
    vnc_loop.abort();
    teardown(stack).await;
    Ok(())
}

/// Bind the persistent listeners: urc-web (16080), the coordinator loop, and the
/// two TLS tunnels (unless `insecure`). Returns the live serving stack.
async fn spawn_serving_stack(
    config: &AgentConfig,
    session: &crate::session::SessionInfo,
    shutdown_rx: &watch::Receiver<bool>,
) -> Result<ServingStack> {
    // Unified web app: serves SPA, WebSocket-to-VNC bridge, files API, clipboard helper.
    // The WS bridge connects to 5900 on demand, so this binds even with VNC down.
    let web_handle = if let Some(root) = &config.files_root {
        let bind: std::net::SocketAddr =
            ([127, 0, 0, 1], urc_common::DEFAULT_WEB_INTERNAL_PORT).into();
        // TODO: if the VNC child later re-detects a different display, the web
        // clipboard keeps using this initial session. Acceptable for now.
        let desktop = urc_web::DesktopSession {
            username: session.username.clone(),
            display: session.display.clone().unwrap_or_else(|| ":0".to_string()),
            xauthority: session.xauthority.clone(),
        };
        Some(urc_web::spawn_web_server(PathBuf::from(root), bind, VNC_PORT, desktop).await?)
    } else {
        None
    };

    let use_coordinator = !config.coordinator_url.trim().is_empty();
    let coordinator = Arc::new(CoordinatorClient::new(config.clone(), VNC_PORT));
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
        // Verify the tunnels can be built (cert generation / config) before
        // committing to the supervised loops, so a hard misconfiguration still
        // surfaces as a startup error rather than an endless retry.
        TlsTunnel::new(config, config.listen_tls_port, VNC_PORT, "vnc")?;
        TlsTunnel::new(
            config,
            config.listen_web_tls_port,
            urc_common::DEFAULT_WEB_INTERNAL_PORT,
            "web",
        )?;

        // Supervise each tunnel independently: if serve() returns (Ok or Err),
        // re-create and re-serve after a short backoff so a transient
        // accept()/bind error self-heals without a full process restart. This is
        // NOT coupled to the VNC loop and must never abort web serving.
        let vnc_cfg = config.clone();
        let v = tokio::spawn(supervise_tunnel(
            vnc_cfg,
            config.listen_tls_port,
            VNC_PORT,
            "vnc",
            shutdown_rx.clone(),
        ));
        let web_cfg = config.clone();
        let w = tokio::spawn(supervise_tunnel(
            web_cfg,
            config.listen_web_tls_port,
            urc_common::DEFAULT_WEB_INTERNAL_PORT,
            "web",
            shutdown_rx.clone(),
        ));
        (Some(v), Some(w))
    } else {
        (None, None)
    };

    info!(
        web_internal = urc_common::DEFAULT_WEB_INTERNAL_PORT,
        "persistent listeners bound — serving regardless of VNC state"
    );

    Ok(ServingStack {
        coordinator,
        coord_handle,
        web_handle,
        vnc_tls_handle,
        web_tls_handle,
    })
}

/// Supervise one TLS tunnel for the agent's lifetime. `TlsTunnel::serve()`
/// normally runs forever; if it returns (Ok or Err) the tunnel is dead, so we
/// re-create and re-serve it after a short capped backoff. This self-heals a
/// transient accept()/bind error (e.g. the external web TLS port 15901 briefly
/// failing) WITHOUT a full process restart, and is shutdown-aware so it does not
/// spin after teardown. It is fully independent of the VNC loop and never
/// touches the persistent listeners.
async fn supervise_tunnel(
    config: AgentConfig,
    listen_port: u16,
    local_port: u16,
    label: &'static str,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut attempt: u32 = 0;
    loop {
        if *shutdown_rx.borrow() {
            return;
        }

        match TlsTunnel::new(&config, listen_port, local_port, label) {
            Ok(tunnel) => match tunnel.serve().await {
                Ok(()) => warn!(label, listen_port, "TLS tunnel returned — re-serving"),
                Err(e) => warn!(label, listen_port, error = %e, "TLS tunnel stopped — re-serving"),
            },
            Err(e) => {
                warn!(label, listen_port, error = %e, "cannot build TLS tunnel — retrying");
            }
        }

        attempt = attempt.saturating_add(1);
        let pause = TUNNEL_RETRY_SECS
            .saturating_mul(1u64 << attempt.min(3).saturating_sub(1))
            .min(TUNNEL_RETRY_MAX_SECS);
        if sleep_or_shutdown(pause, &mut shutdown_rx).await {
            return;
        }
    }
}

/// Independent VNC supervisor: start the backend; poll its health; on failure,
/// stop + reap + back off + retry — WITHOUT touching the persistent listeners.
/// Sets `vnc_health` for the status loop to report (informational only).
async fn vnc_supervisor_loop(
    config: AgentConfig,
    initial_session: crate::session::SessionInfo,
    vnc_health: Arc<std::sync::atomic::AtomicBool>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    use std::sync::atomic::Ordering;
    let mut attempt: u32 = 0;

    loop {
        if *shutdown_rx.borrow() {
            return;
        }
        attempt = attempt.saturating_add(1);

        // Re-detect the session each attempt so the VNC child can recover if the
        // display changed. Fall back to the initial session if detection fails.
        let session = SessionDetector::detect(config.backend).unwrap_or_else(|e| {
            warn!(error = %e, "session re-detect failed — reusing initial session for VNC");
            initial_session.clone()
        });

        let backend = match BackendManager::new(&config, &session) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "cannot build VNC backend — retrying");
                let pause = backoff_secs(attempt);
                if sleep_or_shutdown(pause, &mut shutdown_rx).await {
                    return;
                }
                continue;
            }
        };

        match backend.start().await {
            Ok(port) => {
                attempt = 0;
                vnc_health.store(true, Ordering::Relaxed);
                info!(port, "VNC backend up — supervising health");
                // Poll health until VNC drops or shutdown.
                let mut tick = interval(Duration::from_secs(VNC_HEALTH_TICK_SECS));
                tick.tick().await;
                loop {
                    tokio::select! {
                        _ = tick.tick() => {
                            if !backend.health_check().await {
                                warn!("VNC backend health check failed — restarting VNC only");
                                vnc_health.store(false, Ordering::Relaxed);
                                break;
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                backend.stop().await;
                                return;
                            }
                        }
                    }
                }
                // VNC dropped: stop + brief reap, then loop to restart it.
                backend.stop().await;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => {
                vnc_health.store(false, Ordering::Relaxed);
                let msg = e.to_string();
                let saturated = msg.contains("saturated")
                    || msg.contains("Maximum number of clients reached");
                // Reap stale VNC procs so the display can drain (start() already
                // reaps, but ensure it after a failure too).
                backend.stop().await;

                let pause = if saturated {
                    warn!(
                        error = %msg,
                        cooldown_secs = SATURATION_COOLDOWN_SECS,
                        "X display saturated — long cooldown so it can drain"
                    );
                    // The saturation cooldown is a fixed wait, not part of the
                    // exponential backoff. Reset the attempt counter so a later
                    // transient failure starts from the short backoff instead of
                    // inheriting a maxed-out 300s pause from saturation churn.
                    attempt = 1;
                    SATURATION_COOLDOWN_SECS
                } else {
                    let p = backoff_secs(attempt);
                    warn!(error = %msg, pause_secs = p, "VNC start failed — backing off");
                    p
                };

                if sleep_or_shutdown(pause, &mut shutdown_rx).await {
                    return;
                }
            }
        }
    }
}

/// Exponential backoff capped at `RECOVERY_PAUSE_MAX_SECS`.
fn backoff_secs(attempt: u32) -> u64 {
    let shift = attempt.min(6).saturating_sub(1);
    RECOVERY_PAUSE_SECS
        .saturating_mul(1u64 << shift)
        .min(RECOVERY_PAUSE_MAX_SECS)
}

/// Sleep `secs`, returning early if shutdown is requested. Returns `true` if the
/// caller should exit (shutdown observed).
async fn sleep_or_shutdown(secs: u64, shutdown_rx: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(secs)) => *shutdown_rx.borrow(),
        _ = shutdown_rx.changed() => *shutdown_rx.borrow(),
    }
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

/// Status/liveness loop. Replaces the old fatal health loop: it NEVER tears
/// anything down. Liveness = web/files port reachable AND coordinator ok (if used).
/// VNC state (`vnc_port_open`) is reported for information only.
async fn status_loop(
    config: &AgentConfig,
    coordinator: &Arc<CoordinatorClient>,
    vnc_health: &Arc<std::sync::atomic::AtomicBool>,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    use std::sync::atomic::Ordering;
    let mut tick = interval(Duration::from_secs(STATUS_TICK_SECS));
    let needs_coordinator = !config.coordinator_url.trim().is_empty();

    loop {
        let web_ok = tcp_open(urc_common::DEFAULT_WEB_INTERNAL_PORT).await;
        let vnc_port_open = vnc_health.load(Ordering::Relaxed);
        let coord_ok = !needs_coordinator || coordinator.is_connected();

        let healthy = web_ok && coord_ok;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let status = AgentStatus {
            healthy,
            session_detected: true,
            vnc_port_open,
            coordinator_connected: coord_ok,
            files_port_open: web_ok,
            backend: Some(format!("{:?}", config.backend)),
            display: None,
            last_error: if healthy {
                if vnc_port_open {
                    None
                } else {
                    Some("vnc=down (serving unaffected)".to_string())
                }
            } else {
                Some(format!(
                    "web={} coordinator={}",
                    if web_ok { "ok" } else { "down" },
                    if coord_ok { "ok" } else { "down" }
                ))
            },
            updated_at_unix: now,
            supervisor_cycle: 0,
        };
        let _ = status.write(std::path::Path::new(STATUS_PATH));

        if !healthy {
            warn!(web_ok, coord_ok, "serving layer unhealthy");
        }

        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return;
                }
            }
        }
    }
}

async fn teardown(mut stack: ServingStack) {
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
