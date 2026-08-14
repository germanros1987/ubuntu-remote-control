//! GNOME Wayland backend via gnome-remote-desktop VNC (grdctl).

use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{sleep, Duration};
use tracing::info;

use super::VncBackend;
use crate::session::SessionInfo;
use urc_common::AgentConfig;

/// GNOME Remote Desktop VNC default port
const LOCAL_PORT: u16 = 5900;

pub struct GnomeBackend {
    username: String,
    password_file: Option<String>,
}

impl GnomeBackend {
    pub fn new(config: &AgentConfig, session: &SessionInfo) -> Result<Self> {
        if which_grdctl().is_none() {
            bail!("grdctl not found. Install: apt install gnome-remote-desktop");
        }

        Ok(Self {
            username: session.username.clone(),
            password_file: config.vnc_password_file.clone(),
        })
    }

    async fn configure_grd(&self) -> Result<()> {
        let password = read_vnc_password(&self.password_file)?;

        run_as_user(
            &self.username,
            &["grdctl", "vnc", "set-auth-method", "password"],
        )
        .await?;
        run_as_user(
            &self.username,
            &["grdctl", "vnc", "set-password", &password],
        )
        .await?;
        run_as_user(&self.username, &["grdctl", "vnc", "disable-view-only"]).await?;
        run_as_user(&self.username, &["grdctl", "vnc", "enable"]).await?;

        // Enable systemd user service
        run_as_user(
            &self.username,
            &[
                "systemctl",
                "--user",
                "enable",
                "--now",
                "gnome-remote-desktop.service",
            ],
        )
        .await?;

        info!("configured gnome-remote-desktop VNC backend");
        Ok(())
    }
}

#[async_trait::async_trait]
impl VncBackend for GnomeBackend {
    fn local_port(&self) -> u16 {
        LOCAL_PORT
    }

    async fn start(&self) -> Result<()> {
        self.configure_grd().await?;

        for _ in 0..30 {
            sleep(Duration::from_secs(1)).await;
            if self.health_check().await? {
                return Ok(());
            }
        }
        bail!(
            "gnome-remote-desktop VNC not reachable on port {LOCAL_PORT}. \
             Check: journalctl --user -u gnome-remote-desktop.service"
        )
    }

    async fn stop(&self) -> Result<()> {
        let _ = run_as_user(&self.username, &["grdctl", "vnc", "disable"]).await;
        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(tokio::net::TcpStream::connect(("127.0.0.1", LOCAL_PORT))
            .await
            .is_ok())
    }
}

fn which_grdctl() -> Option<String> {
    std::process::Command::new("which")
        .arg("grdctl")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| "grdctl".into())
}

fn read_vnc_password(path: &Option<String>) -> Result<String> {
    if let Some(p) = path {
        let bytes = std::fs::read(p).context("read vnc password file")?;
        if bytes.len() >= 8 {
            return Ok(String::from_utf8_lossy(&bytes[..8]).to_string());
        }
    }
    Ok("urc-default".to_string())
}

async fn run_as_user(user: &str, args: &[&str]) -> Result<()> {
    let mut cmd = Command::new("runuser");
    cmd.args(["-u", user, "--"]);
    cmd.arg(args[0]);
    for a in &args[1..] {
        cmd.arg(a);
    }
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("runuser")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("command failed {:?}: {}", args, stderr);
    }
    Ok(())
}
