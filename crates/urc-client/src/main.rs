//! Ubuntu Remote Control client — connect via coordinator and launch VNC viewer.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::info;
use tracing_subscriber::EnvFilter;
use urc_common::{parse_ws_message, to_ws_message, ClientMessage, CoordinatorMessage};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "urc-client", about = "Ubuntu Remote Control client")]
struct Cli {
    #[arg(long, default_value = "ws://127.0.0.1:21150/ws/client")]
    coordinator: String,

    #[arg(long, default_value = "changeme")]
    token: String,

    /// Map Mac Command key to Super/Windows key when using TigerVNC
    #[arg(long, default_value_t = true)]
    mac_cmd_to_super: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List online hosts
    Hosts,
    /// Connect to a host by ID
    Connect {
        host_id: String,
        /// Local port for VNC viewer (default 15900)
        #[arg(long, default_value_t = 15900)]
        local_port: u16,
        /// VNC viewer binary
        #[arg(long, default_value = "vncviewer")]
        viewer: String,
        /// Password file for viewer
        #[arg(long)]
        password_file: Option<PathBuf>,
    },
    /// Upload a file to remote urc-files service (requires port-forward)
    Upload {
        host: String,
        local_path: PathBuf,
        remote_path: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Hosts => list_hosts(&cli).await,
        Commands::Connect {
            ref host_id,
            local_port,
            ref viewer,
            ref password_file,
        } => connect_host(&cli, host_id, local_port, viewer, password_file.as_deref()).await,
        Commands::Upload { .. } => {
            anyhow::bail!("use connect first; file upload via curl to forwarded files port")
        }
    }
}

async fn list_hosts(cli: &Cli) -> Result<()> {
    let base = cli.coordinator.replace("/ws/client", "");
    let url = format!("{base}/hosts");
    let body = reqwest_get(&url).await?;
    println!("{body}");
    Ok(())
}

async fn connect_host(
    cli: &Cli,
    host_id: &str,
    local_port: u16,
    viewer: &str,
    password_file: Option<&std::path::Path>,
) -> Result<()> {
    let (ws, _) = connect_async(&cli.coordinator).await.context("coordinator")?;
    let (mut write, mut read) = ws.split();

    let connect = ClientMessage::Connect {
        host_id: host_id.to_string(),
        token: cli.token.clone(),
    };
    write
        .send(Message::Text(to_ws_message(&connect)?.into()))
        .await?;

    let mut session_id = None;
    let mut direct_tailscale = None;

    while let Some(Ok(Message::Text(text))) = read.next().await {
        if let Ok(CoordinatorMessage::ConnectOk {
            session_id: sid,
            relay_mode,
        }) = parse_ws_message(&text)
        {
            session_id = Some(sid);
            info!(?relay_mode, %sid, "connection approved");
            break;
        } else if let Ok(CoordinatorMessage::ConnectErr { reason }) = parse_ws_message(&text) {
            anyhow::bail!("connect failed: {reason}");
        } else if text.starts_with("Direct connect:") {
            direct_tailscale = Some(text.clone());
        } else if let Ok(CoordinatorMessage::RelayHint { session_id: sid, .. }) =
            parse_ws_message(&text)
        {
            session_id = Some(sid);
        }
    }

    let session_id = session_id.context("no session from coordinator")?;

    if let Some(hint) = direct_tailscale {
        info!(%hint, "use direct path when on Tailscale");
    }

    // Local port forward via relay websocket
    start_local_forward(&cli.coordinator, session_id, local_port).await?;

    let mut cmd = Command::new(viewer);
    cmd.arg(format!("127.0.0.1:{local_port}"));
    if cli.mac_cmd_to_super {
        cmd.env("URC_MAC_CMD_TO_SUPER", "1");
    }
    if let Some(pw) = password_file {
        cmd.arg("-PasswordFile").arg(pw);
    }

    info!(%local_port, "launching VNC viewer");
  info!("Mac tip: Cmd maps to Super when URC_MAC_CMD_TO_SUPER=1; use TigerVNC for best clipboard");

    let status = cmd.status().context("vncviewer")?;
    if !status.success() {
        anyhow::bail!("viewer exited with {status}");
    }
    Ok(())
}

async fn start_local_forward(coordinator: &str, session_id: Uuid, local_port: u16) -> Result<()> {
    let base = coordinator
        .replace("wss://", "ws://")
        .replace("https://", "ws://")
        .replace("http://", "ws://")
        .replace("/ws/client", "");

    let tunnel_url = format!("{base}/tunnel/client/{session_id}");
    let listener = TcpListener::bind(("127.0.0.1", local_port)).await?;

    let tunnel_url_clone = tunnel_url.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut local, _)) = listener.accept().await else {
                continue;
            };
            let url = tunnel_url_clone.clone();
            tokio::spawn(async move {
                if let Err(e) = pipe_relay(&url, &mut local).await {
                    tracing::debug!(error = %e, "relay pipe ended");
                }
            });
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    Ok(())
}

async fn pipe_relay(url: &str, local: &mut TcpStream) -> Result<()> {
    let (ws, _) = connect_async(url).await?;
    let (mut ws_sink, mut ws_source) = ws.split();
    let (mut lr, mut lw) = local.split();

    let w2l = async {
        while let Some(msg) = ws_source.next().await {
            match msg? {
                Message::Binary(d) => lw.write_all(&d).await?,
                Message::Text(t) => {
                    if let Ok(bytes) = base64_decode_payload(&t) {
                        lw.write_all(&bytes).await?;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok::<(), anyhow::Error>(())
    };

    let l2w = async {
        let mut buf = [0u8; 8192];
        loop {
            let n = lr.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            ws_sink
                .send(Message::Binary(buf[..n].to_vec().into()))
                .await?;
        }
        Ok::<(), anyhow::Error>(())
    };

    tokio::try_join!(w2l, l2w)?;
    Ok(())
}

fn base64_decode_payload(text: &str) -> Result<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    if let Ok(msg) = parse_ws_message::<serde_json::Value>(text) {
        if let Some(data) = msg.get("data").and_then(|d| d.as_str()) {
            return Ok(STANDARD.decode(data)?);
        }
    }
    Ok(vec![])
}

async fn reqwest_get(url: &str) -> Result<String> {
    let output = std::process::Command::new("curl")
        .args(["-sf", url])
        .output()
        .context("curl")?;
    if !output.status.success() {
        anyhow::bail!("HTTP request failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
