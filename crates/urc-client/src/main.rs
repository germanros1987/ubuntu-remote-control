//! Ubuntu Remote Control client.
//!
//! Resolves a Tailscale peer running `urc-agent`, starts a local TLS-wrapping
//! TCP forwarder pointed at the agent's unified web port, then opens the user's
//! default browser at the local URL. The browser app (noVNC + files panel,
//! served by `urc-web` on the agent) is the entire UI.

mod sessions;
mod tls_forward;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::process::{Command, Stdio};
use tracing::info;
use tracing_subscriber::EnvFilter;
use urc_common::DEFAULT_WEB_TLS_PORT;

#[derive(Parser, Debug)]
#[command(name = "urc-client", about = "Ubuntu Remote Control client")]
struct Cli {
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
        /// Local port to bind the proxy on (default: pick a free high port)
        #[arg(long)]
        local_port: Option<u16>,
        /// Don't auto-open the browser; just print the URL
        #[arg(long, default_value_t = false)]
        no_open: bool,
        /// Run the tunnel in the background; return immediately
        #[arg(long, default_value_t = false)]
        bg: bool,
        /// Internal flag — set on the detached child so it knows to clean its
        /// session file on exit. Users should never pass this directly.
        #[arg(long, hide = true, default_value_t = false)]
        detached_child: bool,
    },

    /// List backgrounded tunnel sessions
    Sessions,

    /// Stop a backgrounded tunnel by host name or pid
    Kill {
        /// Host name (as shown by `urc sessions`) or pid
        target: String,
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
            host_id,
            local_port,
            no_open,
            bg,
            detached_child,
        } => connect_host(&host_id, local_port, no_open, bg, detached_child).await,
        Commands::Sessions => list_sessions().await,
        Commands::Kill { target } => kill_session(&target).await,
    }
}

async fn list_hosts() -> Result<()> {
    let json = tokio::task::spawn_blocking(urc_common::tailscale::status_json)
        .await
        .context("tailscale status")??;
    urc_common::tailscale::ensure_local_running(&json)?;
    let peers = urc_common::tailscale::list_peers(&json);

    if peers.is_empty() {
        println!("No other machines on your tailnet.");
        println!("Install URC on each PC you want to control, sign in with the same Tailscale account, then run urc hosts again.");
        return Ok(());
    }

    println!("{:<24} {:<16} STATUS", "NAME", "TAILSCALE IP");
    for p in &peers {
        let status = if p.online { "online" } else { "offline" };
        println!("{:<24} {:<16} {}", p.name, p.ipv4, status);
    }
    println!();
    println!("Connect: urc connect NAME");
    Ok(())
}

async fn connect_host(
    host_id: &str,
    local_port: Option<u16>,
    no_open: bool,
    bg: bool,
    detached_child: bool,
) -> Result<()> {
    if bg && !detached_child {
        return spawn_detached(host_id, local_port).await;
    }

    let json = tokio::task::spawn_blocking(urc_common::tailscale::status_json)
        .await
        .context("tailscale status")??;
    urc_common::tailscale::ensure_local_running(&json)?;
    let peers = urc_common::tailscale::list_peers(&json);

    let Some(peer) = urc_common::tailscale::resolve_peer(&peers, host_id) else {
        anyhow::bail!(
            "no machine named '{host_id}' on your tailnet.\nRun: urc hosts\nMachines must use the same Tailscale account and have URC installed."
        );
    };

    if !peer.online {
        anyhow::bail!(
            "'{}' is offline on Tailscale. Wake the machine or check it is logged in.",
            peer.name
        );
    }

    if !detached_child {
        println!(
            "Checking {} ({}) on port {DEFAULT_WEB_TLS_PORT}…",
            peer.name, peer.ipv4
        );
    }
    tls_forward::preflight_remote_web(&peer.ipv4, DEFAULT_WEB_TLS_PORT)
        .await
        .with_context(|| format!("verify urc-agent on {}", peer.name))?;

    let local_port = local_port.unwrap_or_else(tls_forward::pick_free_port);
    tls_forward::spawn_tls_forward(&peer.ipv4, DEFAULT_WEB_TLS_PORT, local_port).await?;

    let url = format!("http://localhost:{local_port}/");
    info!(name = %peer.name, ip = %peer.ipv4, %url, "tunnel up");

    if detached_child {
        // Parent already wrote the session file with our PID + URL and printed
        // the user-facing summary. Just block until SIGTERM and clean up.
        let host_for_cleanup = peer.name.clone();
        tokio::signal::ctrl_c().await.context("wait for SIGTERM")?;
        let _ = sessions::remove(&host_for_cleanup);
        return Ok(());
    }

    println!(
        "Tunnel ready: 127.0.0.1:{local_port} → {} ({}):{DEFAULT_WEB_TLS_PORT}",
        peer.name, peer.ipv4
    );
    println!("URL: {url}");

    if !no_open {
        open_browser(&url);
        println!("Opened in your default browser. Press Ctrl+C to disconnect.");
    } else {
        println!("Open the URL above in any browser. Press Ctrl+C to disconnect.");
    }

    tokio::signal::ctrl_c()
        .await
        .context("wait for disconnect")?;
    Ok(())
}

/// Resolve the peer + pick a port up front so the URL is known before forking,
/// then exec ourselves with `--detached-child`, detach stdio, and exit. The
/// child inherits no controlling terminal and keeps the tunnel alive.
async fn spawn_detached(host_id: &str, local_port: Option<u16>) -> Result<()> {
    let json = tokio::task::spawn_blocking(urc_common::tailscale::status_json)
        .await
        .context("tailscale status")??;
    urc_common::tailscale::ensure_local_running(&json)?;
    let peers = urc_common::tailscale::list_peers(&json);

    let Some(peer) = urc_common::tailscale::resolve_peer(&peers, host_id) else {
        anyhow::bail!("no machine named '{host_id}' on your tailnet.");
    };
    if !peer.online {
        anyhow::bail!("'{}' is offline on Tailscale.", peer.name);
    }
    if let Some(existing) = sessions::find(&peer.name)? {
        anyhow::bail!(
            "'{}' already has a running session (pid {}, {}).\n\
             Stop it with: urc kill {}",
            peer.name,
            existing.pid,
            existing.url,
            peer.name
        );
    }

    let port = local_port.unwrap_or_else(tls_forward::pick_free_port);
    let exe = std::env::current_exe().context("current_exe")?;
    let mut cmd = Command::new(&exe);
    cmd.arg("connect").arg(&peer.name);
    cmd.arg("--local-port").arg(port.to_string());
    cmd.arg("--no-open");
    cmd.arg("--detached-child");
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Detach into its own process group so closing the terminal doesn't
        // SIGHUP the child.
        cmd.process_group(0);
    }

    let child = cmd.spawn().context("spawn background tunnel")?;
    let pid = child.id();

    let url = format!("http://localhost:{port}/");
    sessions::write(&sessions::Session {
        host: peer.name.clone(),
        ip: peer.ipv4.clone(),
        pid,
        local_port: port,
        url: url.clone(),
        started_at_unix: sessions::now(),
    })?;

    println!("Tunnel detached: {} (pid {pid})", peer.name);
    println!("URL: {url}");
    println!();
    println!("  Stop:  urc kill {}", peer.name);
    println!("  List:  urc sessions");
    Ok(())
}

async fn list_sessions() -> Result<()> {
    let sessions = sessions::list()?;
    if sessions.is_empty() {
        println!("No background sessions. Start one with: urc connect HOST --bg");
        return Ok(());
    }
    println!(
        "{:<20} {:<16} {:<8} {:<10} URL",
        "HOST", "TAILSCALE IP", "PID", "UPTIME"
    );
    for s in &sessions {
        println!(
            "{:<20} {:<16} {:<8} {:<10} {}",
            s.host,
            s.ip,
            s.pid,
            sessions::human_age(s.started_at_unix),
            s.url
        );
    }
    Ok(())
}

async fn kill_session(target: &str) -> Result<()> {
    let s = sessions::kill(target)?;
    println!("Killed session '{}' (pid {})", s.host, s.pid);
    Ok(())
}

fn open_browser(url: &str) {
    let cmd = if std::env::consts::OS == "macos" {
        "open"
    } else if std::env::consts::OS == "windows" {
        "start"
    } else {
        "xdg-open"
    };
    let _ = Command::new(cmd).arg(url).status();
}
