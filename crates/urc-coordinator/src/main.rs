//! Ubuntu Remote Control coordinator — rendezvous and relay.

mod relay;
mod state;

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;
use tracing_subscriber::EnvFilter;
use urc_common::{
    parse_ws_message, to_ws_message, AgentMessage, ClientMessage, CoordinatorMessage, RelayMode,
    TunnelTarget,
};
use uuid::Uuid;

use state::AppState;

#[derive(Parser, Debug)]
#[command(name = "urc-coordinator")]
struct Cli {
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,

    #[arg(long, default_value_t = urc_common::DEFAULT_COORDINATOR_PORT)]
    port: u16,

    #[arg(long, default_value = "changeme")]
    shared_secret: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("urc_coordinator=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let state = Arc::new(AppState::new(cli.shared_secret.clone()));

    let app = Router::new()
        .route("/ws/agent", get(agent_ws))
        .route("/ws/client", get(client_ws))
        .route("/tunnel/agent/{session_id}", get(agent_tunnel))
        .route("/tunnel/client/{session_id}", get(client_tunnel))
        .route("/hosts", get(list_hosts))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", cli.bind, cli.port).parse()?;
    info!(%addr, "urc-coordinator listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn list_hosts(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let hosts = state.list_hosts().await;
    axum::Json(hosts)
}

async fn agent_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_agent(socket, state))
}

async fn client_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_client(socket, state))
}

async fn handle_agent(socket: WebSocket, state: Arc<AppState>) {
    let (sink, mut stream) = socket.split();
    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<Message>();
    let mut host_id = String::new();

    let mut sink = sink;
    let forward = tokio::spawn(async move {
        while let Some(msg) = control_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(Message::Text(text))) = stream.next().await {
        match parse_ws_message::<AgentMessage>(&text) {
            Ok(AgentMessage::Register {
                host_id: id,
                token,
                tailscale_ip,
                vnc_local_port,
            }) => {
                if !state.verify_token(&token) {
                    return;
                }
                host_id = id.clone();
                state
                    .register_agent(id.clone(), tailscale_ip, vnc_local_port, control_tx.clone())
                    .await;

                let ok = CoordinatorMessage::Registered { host_id: id };
                let _ = control_tx.send(Message::Text(to_ws_message(&ok).unwrap().into()));
            }
            Ok(AgentMessage::Heartbeat { host_id: id }) => {
                state.touch_agent(&id).await;
            }
            _ => {}
        }
    }

    forward.abort();
    if !host_id.is_empty() {
        state.unregister_agent(&host_id).await;
    }
}

async fn handle_client(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();

    while let Some(Ok(Message::Text(text))) = stream.next().await {
        if let Ok(ClientMessage::Connect { host_id, token }) = parse_ws_message(&text) {
            if !state.verify_token(&token) {
                let err = CoordinatorMessage::ConnectErr {
                    reason: "invalid token".into(),
                };
                let _ = sink
                    .send(Message::Text(to_ws_message(&err).unwrap().into()))
                    .await;
                continue;
            }

            let Some(agent) = state.get_agent(&host_id).await else {
                let err = CoordinatorMessage::ConnectErr {
                    reason: format!("host '{host_id}' not online"),
                };
                let _ = sink
                    .send(Message::Text(to_ws_message(&err).unwrap().into()))
                    .await;
                continue;
            };

            let session_id = Uuid::new_v4();
            let relay_mode = if agent.tailscale_ip.is_some() {
                RelayMode::Direct
            } else {
                RelayMode::Relayed
            };

            let tunnel_ready = AgentMessage::TunnelReady {
                session_id,
                target: TunnelTarget::Vnc,
            };
            if let Some(agent_tx) = state.agent_control_tx(&host_id).await {
                let msg = Message::Text(to_ws_message(&tunnel_ready).unwrap().into());
                let _ = agent_tx.send(msg);
            }

            state.relay.create_session(session_id).await;

            let ok = CoordinatorMessage::ConnectOk {
                session_id,
                relay_mode,
            };
            let _ = sink
                .send(Message::Text(to_ws_message(&ok).unwrap().into()))
                .await;

            if let Some(ts_ip) = agent.tailscale_ip {
                let hint = format!(
                    "Direct connect: vncviewer {ts_ip}:{} (TLS port {})",
                    agent.vnc_local_port,
                    urc_common::DEFAULT_TLS_LISTEN_PORT
                );
                let _ = sink.send(Message::Text(hint.into())).await;
            } else {
                let hint = CoordinatorMessage::RelayHint {
                    session_id,
                    coordinator_addr: format!(
                        "ws://127.0.0.1:{}",
                        urc_common::DEFAULT_COORDINATOR_PORT
                    ),
                };
                let _ = sink
                    .send(Message::Text(to_ws_message(&hint).unwrap().into()))
                    .await;
            }
        }
    }
}

async fn agent_tunnel(
    ws: WebSocketUpgrade,
    Path(session_id): Path<Uuid>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        state.relay.attach_agent(session_id, socket).await;
    })
}

async fn client_tunnel(
    ws: WebSocketUpgrade,
    Path(session_id): Path<Uuid>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        state.relay.attach_client(session_id, socket).await;
    })
}
