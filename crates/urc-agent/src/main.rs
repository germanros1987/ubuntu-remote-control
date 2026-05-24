//! Ubuntu Remote Control agent.

mod backend;
mod config;
mod coordinator;
mod health;
mod session;
mod share;
mod supervisor;
mod tailscale;
mod tunnel;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;
use urc_common::AgentConfig;

#[derive(Parser, Debug)]
#[command(name = "urc-agent", about = "Ubuntu Remote Control agent")]
struct Cli {
    /// Path to config file
    #[arg(long, default_value = "/etc/urc/agent.toml")]
    config: PathBuf,

    /// Run without TLS (LAN only)
    #[arg(long)]
    insecure: bool,

    /// Generate default config and exit
    #[arg(long)]
    init_config: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the agent supervisor (default for systemd)
    Run,
    /// Exit 0 if healthy (for watchdog scripts)
    Health,
    /// Print JSON status
    Status,
    /// Print a QR code + urc:// deep link to pair the Android client
    Share,
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
        .with_env_filter(EnvFilter::from_default_env().add_directive("urc_agent=info".parse()?))
        .init();

    let cli = Cli::parse();

    if cli.init_config {
        let cfg = AgentConfig::default();
        let text = toml::to_string_pretty(&cfg)?;
        println!("{text}");
        return Ok(());
    }

    let mut config = config::load_config(&cli.config)?;
    if cli.insecure {
        config.insecure = true;
        config.encryption = urc_common::EncryptionMode::None;
    }

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => supervisor::run_supervisor(config).await,
        Command::Health => health::run_health_check(&config).await,
        Command::Status => health::run_status_cmd(&config).await,
        Command::Share => share::run_share(),
    }
}
