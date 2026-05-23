//! Outbound WebSocket to coordinator for registration and relay tunnels.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};
use urc_common::{parse_ws_message, to_ws_message, AgentMessage, AgentConfig, TunnelTarget};
use uuid::Uuid;

pub struct CoordinatorClient {
    config: AgentConfig,
    vnc_port: u16,
    active_tunnels: Arc<Mutex<HashMap<Uuid, tokio::task::JoinHandle<()>>>>,
    connected: Arc<AtomicBool>,
}

impl CoordinatorClient {
    pub fn new(config: AgentConfig, vnc_port: u16) -> Self {
        Self {
            config,
            vnc_port,
            active_tunnels: Arc::new(Mutex::new(HashMap::new())),
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub async fn abort_all_tunnels(&self) {
        let mut tunnels = self.active_tunnels.lock().await;
        for (_, handle) in tunnels.drain() {
            handle.abort();
        }
    }

    pub async fn run_loop(self: Arc<Self>) {
        loop {
            if let Err(e) = self.connect_once().await {
                warn!(error = %e, "coordinator disconnected, retrying in 5s");
                self.connected.store(false, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    async fn connect_once(self: &Arc<Self>) -> Result<()> {
        self.connected.store(false, Ordering::Relaxed);

        if self.config.coordinator_url.trim().is_empty() {
            info!("coordinator disabled — Tailscale-only mode");
            self.connected.store(true, Ordering::Relaxed);
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }

        let url = self
            .config
            .coordinator_url
            .replace("https://", "wss://")
            .replace("http://", "ws://");

        let (ws, _) = connect_async(&url).await.context("connect coordinator")?;
        info!(%url, "connected to coordinator");

        let host_id = self
            .config
            .host_id
            .clone()
            .unwrap_or_else(|| "unknown".into());
        let token = self.config.token.clone().unwrap_or_default();

        let tailscale_ip = if self.config.tailscale.enabled {
            crate::tailscale::detect_ip().await
        } else {
            None
        };

        let register = AgentMessage::Register {
            host_id: host_id.clone(),
            token,
            tailscale_ip,
            vnc_local_port: self.vnc_port,
            files_local_port: urc_common::DEFAULT_FILES_PORT,
        };

        let (mut write, mut read) = ws.split();
        write
            .send(Message::Text(to_ws_message(&register)?.into()))
            .await?;

        self.connected.store(true, Ordering::Relaxed);

        let mut heartbeat = interval(Duration::from_secs(30));
        let vnc_port = self.vnc_port;
        let tunnels = self.active_tunnels.clone();

        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    let hb = AgentMessage::Heartbeat { host_id: host_id.clone() };
                    write.send(Message::Text(to_ws_message(&hb)?.into())).await?;
                }
                msg = read.next() => {
                    let Some(Ok(Message::Text(text))) = msg else {
                        self.connected.store(false, Ordering::Relaxed);
                        anyhow::bail!("coordinator connection closed");
                    };
                    if let Ok(agent_msg) = parse_ws_message::<AgentMessage>(&text) {
                        if let AgentMessage::TunnelReady { session_id, target } = agent_msg {
                            let port = match target {
                                TunnelTarget::Vnc => vnc_port,
                                TunnelTarget::Files => urc_common::DEFAULT_FILES_PORT,
                            };
                            let tunnels_cleanup = tunnels.clone();
                            let handle = tokio::spawn(async move {
                                if let Err(e) = run_tunnel_session(session_id, port).await {
                                    debug!(error = %e, %session_id, "tunnel ended");
                                }
                                tunnels_cleanup.lock().await.remove(&session_id);
                            });
                            tunnels.lock().await.insert(session_id, handle);
                        }
                    }
                }
            }
        }
    }
}

async fn run_tunnel_session(session_id: Uuid, local_port: u16) -> Result<()> {
    let coordinator_host =
        std::env::var("URC_COORDINATOR_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let coordinator_port: u16 = std::env::var("URC_COORDINATOR_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(urc_common::DEFAULT_COORDINATOR_PORT);

    let url = format!(
        "ws://{coordinator_host}:{coordinator_port}/tunnel/agent/{session_id}"
    );
    let (ws, _) = connect_async(&url).await.context("tunnel websocket")?;
    let local = TcpStream::connect(("127.0.0.1", local_port)).await?;

    let (mut ws_sink, mut ws_source) = ws.split();
    let (mut local_read, mut local_write) = local.into_split();

    let ws_to_local = async {
        while let Some(msg) = ws_source.next().await {
            match msg? {
                Message::Binary(data) => {
                    local_write.write_all(&data).await?;
                }
                Message::Text(text) => {
                    if let Ok(AgentMessage::TunnelData { data, .. }) = parse_ws_message(&text) {
                        local_write.write_all(&data).await?;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok::<(), anyhow::Error>(())
    };

    let local_to_ws = async {
        let mut buf = [0u8; 8192];
        loop {
            let n = local_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            ws_sink
                .send(Message::Binary(buf[..n].to_vec().into()))
                .await?;
        }
        Ok::<(), anyhow::Error>(())
    };

    tokio::try_join!(ws_to_local, local_to_ws)?;
    Ok(())
}
