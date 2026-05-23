//! Ubuntu Remote Control client.
//!
//! Resolves a Tailscale peer running `urc-agent`, starts a local TLS-wrapping
//! TCP forwarder pointed at the agent's unified web port, then opens the user's
//! default browser at the local URL. The browser app (noVNC + files panel,
//! served by `urc-web` on the agent) is the entire UI.

mod tls_forward;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::process::Command;
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
        } => connect_host(&host_id, local_port, no_open).await,
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

    println!("{:<24} {:<16} {}", "NAME", "TAILSCALE IP", "STATUS");
    for p in &peers {
        let status = if p.online { "online" } else { "offline" };
        println!("{:<24} {:<16} {}", p.name, p.ipv4, status);
    }
    println!();
    println!("Connect: urc connect NAME");
    Ok(())
}

async fn connect_host(host_id: &str, local_port: Option<u16>, no_open: bool) -> Result<()> {
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

    println!(
        "Checking {} ({}) on port {DEFAULT_WEB_TLS_PORT}…",
        peer.name, peer.ipv4
    );
    tls_forward::preflight_remote_web(&peer.ipv4, DEFAULT_WEB_TLS_PORT)
        .await
        .with_context(|| format!("verify urc-agent on {}", peer.name))?;

    let local_port = local_port.unwrap_or_else(tls_forward::pick_free_port);
    tls_forward::spawn_tls_forward(&peer.ipv4, DEFAULT_WEB_TLS_PORT, local_port).await?;

    let url = format!("http://localhost:{local_port}/");
    info!(name = %peer.name, ip = %peer.ipv4, %url, "tunnel up");
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
