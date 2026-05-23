//! Ubuntu Remote Control client — discover machines on Tailscale and connect.

mod tls_forward;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::Command;
use tracing::info;
use tracing_subscriber::EnvFilter;
use urc_common::{
    parse_ws_message, to_ws_message, ClientMessage, CoordinatorMessage, DEFAULT_TLS_LISTEN_PORT,
};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "urc-client", about = "Ubuntu Remote Control client")]
struct Cli {
    /// Coordinator WebSocket (optional — only for relay fallback)
    #[arg(long, default_value = "")]
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
    /// List machines on your Tailscale network
    Hosts,
    /// Connect to a machine by Tailscale name
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

fn install_rustls_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("install rustls ring crypto provider");
}

#[tokio::main]
async fn main() -> Result<()> {
    install_rustls_provider();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Hosts => list_hosts().await,
        Commands::Connect {
            ref host_id,
            local_port,
            ref viewer,
            ref password_file,
        } => {
            connect_host(
                &cli,
                host_id,
                local_port,
                viewer,
                password_file.as_deref(),
            )
            .await
        }
        Commands::Upload { .. } => {
            anyhow::bail!("use connect first; file upload via curl to forwarded files port")
        }
    }
}

async fn list_hosts() -> Result<()> {
    let json = tokio::task::spawn_blocking(urc_common::tailscale::status_json)
        .await
        .context("tailscale status")??;
    let peers = urc_common::tailscale::list_peers(&json);

    if peers.is_empty() {
        println!("No other machines on your tailnet.");
        println!("Install URC on each PC you want to control, sign in with the same Tailscale account, then run urc hosts again.");
        return Ok(());
    }

    println!("{:<24} {:<16} {}", "NAME", "TAILSCALE IP", "STATUS");
    for p in &peers {
        let status = if p.online { "online" } else { "offline" };
        println!("{:<24} {:<16} {}", p.name, p.ipv4, status);
    }
    println!();
    println!("Connect: urc connect NAME");
    Ok(())
}

async fn connect_host(
    cli: &Cli,
    host_id: &str,
    local_port: u16,
    viewer: &str,
    password_file: Option<&std::path::Path>,
) -> Result<()> {
    let json = tokio::task::spawn_blocking(urc_common::tailscale::status_json)
        .await
        .context("tailscale status")??;
    let peers = urc_common::tailscale::list_peers(&json);

    if let Some(peer) = urc_common::tailscale::resolve_peer(&peers, host_id) {
        if !peer.online {
            anyhow::bail!(
                "'{}' is offline on Tailscale. Wake the machine or check it is logged in.",
                peer.name
            );
        }
        info!(name = %peer.name, ip = %peer.ipv4, "connecting via Tailscale");
        println!(
            "Checking {} ({}) on port {}…",
            peer.name, peer.ipv4, DEFAULT_TLS_LISTEN_PORT
        );
        tls_forward::preflight_remote_vnc(&peer.ipv4, DEFAULT_TLS_LISTEN_PORT).await?;
        tls_forward::spawn_tls_forward(&peer.ipv4, DEFAULT_TLS_LISTEN_PORT, local_port).await?;
        tls_forward::probe_local_vnc(local_port).await?;
        println!(
            "Tunnel ready: 127.0.0.1:{local_port} → {} ({})",
            peer.name, peer.ipv4
        );
        launch_viewer(viewer, local_port, cli.mac_cmd_to_super, password_file).await?;
        return Ok(());
    }

    if !cli.coordinator.trim().is_empty() {
        info!("not found on Tailscale, trying coordinator relay");
        return connect_via_coordinator(cli, host_id, local_port, viewer, password_file).await;
    }

    anyhow::bail!(
        "no machine named '{host_id}' on your tailnet.\nRun: urc hosts\nMachines must use the same Tailscale account and have URC installed."
    );
}

/// Find any TigerVNC.app under /Applications regardless of version-specific naming
/// ("TigerVNC Viewer.app" up to 1.15; "TigerVNC.app" from 1.16 onward).
fn glob_tigervnc_app() -> Option<String> {
    let dir = std::fs::read_dir("/Applications").ok()?;
    for entry in dir.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if s.starts_with("TigerVNC") && s.ends_with(".app") {
            let bin = entry.path().join("Contents/MacOS/vncviewer");
            if bin.is_file() {
                return Some(bin.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn find_vncviewer() -> String {
    if let Ok(path) = std::process::Command::new("which").arg("vncviewer").output() {
        if path.status.success() {
            let s = String::from_utf8_lossy(&path.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    for candidate in [
        "/usr/local/bin/vncviewer",
        "/opt/homebrew/bin/vncviewer",
    ] {
        if std::path::Path::new(candidate).is_file()
            || std::path::Path::new(candidate).is_symlink()
        {
            return candidate.to_string();
        }
    }
    if let Some(p) = glob_tigervnc_app() {
        return p;
    }
    "vncviewer".to_string()
}

fn credentials_paths() -> &'static [&'static str] {
    &[
        "/usr/local/etc/urc/credentials",
        "/etc/urc/credentials",
    ]
}

fn read_vnc_password(password_file: Option<&std::path::Path>) -> Option<String> {
    if let Some(pw) = password_file {
        return std::fs::read_to_string(pw)
            .ok()
            .map(|s| s.lines().next().unwrap_or("").to_string())
            .filter(|s| !s.is_empty());
    }
    for creds in credentials_paths() {
        if !std::path::Path::new(creds).is_readable() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(creds) {
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("URC_VNC_PASSWORD=") {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

fn vnc_url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// macOS built-in Screen Sharing (`open vnc://…`) — no TigerVNC install required.
async fn launch_viewer_macos(local_port: u16, password: Option<&str>) -> Result<()> {
    // localhost works more reliably than 127.0.0.1 with Apple's Screen Sharing.
    let url = if let Some(pw) = password.filter(|p| !p.is_empty()) {
        let _ = Command::new("pbcopy").arg(pw).status();
        println!("VNC password copied to clipboard — paste if Screen Sharing prompts.");
        format!(
            "vnc://urc:{}@localhost:{local_port}",
            vnc_url_encode(pw)
        )
    } else {
        format!("vnc://localhost:{local_port}")
    };
    info!(%url, "opening built-in Screen Sharing");
    // Tunnel is verified before we get here; brief pause so Screen Sharing does not race the listener.
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let status = Command::new("open")
        .arg("-a")
        .arg("Screen Sharing")
        .arg(&url)
        .status()
        .context("open Screen Sharing (vnc://)")?;
    if !status.success() {
        Command::new("open")
            .arg(&url)
            .status()
            .context("open vnc:// URL")?;
    }
    println!("Remote desktop opened in Screen Sharing.");
    println!("Press Ctrl+C here when you are done (keeps the tunnel up).");
    tokio::signal::ctrl_c()
        .await
        .context("wait for disconnect")?;
    Ok(())
}

async fn launch_viewer(
    viewer: &str,
    local_port: u16,
    mac_cmd_to_super: bool,
    password_file: Option<&std::path::Path>,
) -> Result<()> {
    let vnc_password = read_vnc_password(password_file);

    if std::env::consts::OS == "macos" && viewer == "vncviewer" {
        // Apple Screen Sharing rejects TigerVNC's RFB handshake on many setups.
        // Prefer TigerVNC Viewer when present; fall through to generic launcher.
        // The Homebrew cask renamed the app from "TigerVNC Viewer.app" (<=1.15) to
        // "TigerVNC.app" (>=1.16), and the installer symlinks the binary to
        // /usr/local/bin/vncviewer so `which vncviewer` resolves it.
        let tigervnc_present = ["/usr/local/bin/vncviewer", "/opt/homebrew/bin/vncviewer"]
            .iter()
            .any(|p| std::path::Path::new(p).is_file() || std::path::Path::new(p).is_symlink())
            || glob_tigervnc_app().is_some();
        if !tigervnc_present {
            return launch_viewer_macos(local_port, vnc_password.as_deref()).await;
        }
    }

    let viewer_bin = if viewer == "vncviewer" {
        find_vncviewer()
    } else {
        viewer.to_string()
    };
    let mut cmd = Command::new(&viewer_bin);
    cmd.arg(format!("127.0.0.1:{local_port}"));
    if mac_cmd_to_super {
        cmd.env("URC_MAC_CMD_TO_SUPER", "1");
    }
    if let Some(pw) = password_file {
        cmd.arg("-PasswordFile").arg(pw);
    } else if let Some(pw) = &vnc_password {
        let tmp = std::env::temp_dir().join("urc-vnc-pass");
        std::fs::write(&tmp, format!("{pw}\n"))?;
        cmd.arg("-PasswordFile").arg(&tmp);
    }

    info!(%local_port, "launching VNC viewer");
    let status = tokio::task::spawn_blocking(move || cmd.status())
        .await
        .context("vncviewer task")??;
    if !status.success() {
        anyhow::bail!("viewer exited with {status}");
    }
    Ok(())
}

trait PathReadable {
    fn is_readable(&self) -> bool;
}

impl PathReadable for std::path::Path {
    fn is_readable(&self) -> bool {
        std::fs::OpenOptions::new().read(true).open(self).is_ok()
    }
}

async fn connect_via_coordinator(
    cli: &Cli,
    host_id: &str,
    local_port: u16,
    viewer: &str,
    password_file: Option<&std::path::Path>,
) -> Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let (ws, _) = connect_async(&cli.coordinator)
        .await
        .context("coordinator")?;
    let (mut write, mut read) = ws.split();

    let connect = ClientMessage::Connect {
        host_id: host_id.to_string(),
        token: cli.token.clone(),
    };
    write
        .send(Message::Text(to_ws_message(&connect)?.into()))
        .await?;

    let mut session_id = None;

    while let Some(Ok(Message::Text(text))) = read.next().await {
        if let Ok(CoordinatorMessage::ConnectOk {
            session_id: sid, ..
        }) = parse_ws_message(&text)
        {
            session_id = Some(sid);
            break;
        } else if let Ok(CoordinatorMessage::ConnectErr { reason }) = parse_ws_message(&text) {
            anyhow::bail!("connect failed: {reason}");
        } else if let Ok(CoordinatorMessage::RelayHint { session_id: sid, .. }) =
            parse_ws_message(&text)
        {
            session_id = Some(sid);
            break;
        }
    }

    let session_id = session_id.context("no session from coordinator")?;
    start_relay_forward(&cli.coordinator, session_id, local_port).await?;
    launch_viewer(viewer, local_port, cli.mac_cmd_to_super, password_file).await
}

async fn start_relay_forward(coordinator: &str, session_id: Uuid, local_port: u16) -> Result<()> {
    use tokio::net::TcpListener;

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

async fn pipe_relay(url: &str, local: &mut tokio::net::TcpStream) -> Result<()> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let (ws, _) = connect_async(url).await?;
    let (mut ws_sink, mut ws_source) = ws.split();
    let (mut lr, mut lw) = local.split();

    let w2l = async {
        while let Some(msg) = ws_source.next().await {
            match msg? {
                Message::Binary(d) => lw.write_all(&d).await?,
                Message::Text(t) => {
                    if let Ok(msg) = parse_ws_message::<serde_json::Value>(&t) {
                        if let Some(data) = msg.get("data").and_then(|d| d.as_str()) {
                            lw.write_all(&STANDARD.decode(data)?).await?;
                        }
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
