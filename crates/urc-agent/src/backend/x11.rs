//! X11 backend using TigerVNC x0vncserver.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

use super::VncBackend;
use crate::session::SessionInfo;
use urc_common::AgentConfig;

const LOCAL_PORT: u16 = 5900;

pub struct X11Backend {
    display: String,
    xauthority: Option<String>,
    password_file: PathBuf,
    child: tokio::sync::Mutex<Option<Child>>,
}

impl X11Backend {
    pub fn new(config: &AgentConfig, session: &SessionInfo) -> Result<Self> {
        let display = session
            .display
            .clone()
            .unwrap_or_else(|| ":0".to_string());

        let password_file = config
            .vnc_password_file
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/etc/urc/vncpasswd"));

        if !password_file.exists() {
            warn!(
                path = %password_file.display(),
                "VNC password file missing; create with vncpasswd or urc-agent setup"
            );
        }

        Ok(Self {
            display,
            xauthority: session.xauthority.clone(),
            password_file,
            child: tokio::sync::Mutex::new(None),
        })
    }
}

#[async_trait::async_trait]
impl VncBackend for X11Backend {
    fn local_port(&self) -> u16 {
        LOCAL_PORT
    }

    async fn start(&self) -> Result<()> {
        if which_x0vncserver().is_none() {
            bail!(
                "x0vncserver not found. Install: apt install tigervnc-standalone-server"
            );
        }

        let mut cmd = Command::new("x0vncserver");
        cmd.arg("-display")
            .arg(&self.display)
            .arg("-rfbport")
            .arg(self.local_port().to_string())
            .arg("-localhost")
            .arg("yes")
            .arg("-UseSHM")
            .arg("1")
            .arg("-FrameRate")
            .arg("60")
            .arg("-SecurityTypes")
            .arg("VncAuth");

        if self.password_file.exists() {
            cmd.arg("-PasswordFile").arg(&self.password_file);
        }

        if let Some(xauth) = &self.xauthority {
            if std::path::Path::new(xauth).exists() {
                cmd.env("XAUTHORITY", xauth);
            }
        }
        cmd.env("DISPLAY", &self.display);

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let child = cmd.spawn().context("spawn x0vncserver")?;
        info!(display = %self.display, port = LOCAL_PORT, "started x0vncserver");

        *self.child.lock().await = Some(child);

        for _ in 0..20 {
            sleep(Duration::from_millis(250)).await;
            if self.health_check().await.unwrap_or(false) {
                return Ok(());
            }
        }

        bail!("x0vncserver failed health check on port {LOCAL_PORT}")
    }

    async fn stop(&self) -> Result<()> {
        if let Some(mut child) = self.child.lock().await.take() {
            child.kill().await.ok();
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(tokio::net::TcpStream::connect(("127.0.0.1", LOCAL_PORT))
            .await
            .is_ok())
    }
}

fn which_x0vncserver() -> Option<String> {
    std::process::Command::new("which")
        .arg("x0vncserver")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}
