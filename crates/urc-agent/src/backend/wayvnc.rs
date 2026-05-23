//! wlroots Wayland backend via wayvnc.

use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Duration};
use tracing::info;

use super::VncBackend;
use crate::session::SessionInfo;
use urc_common::AgentConfig;

const LOCAL_PORT: u16 = 5900;

pub struct WayvncBackend {
    wayland_display: String,
    password_file: Option<String>,
    child: tokio::sync::Mutex<Option<Child>>,
}

impl WayvncBackend {
    pub fn new(config: &AgentConfig, session: &SessionInfo) -> Result<Self> {
        if which_wayvnc().is_none() {
            bail!("wayvnc not found. Install: apt install wayvnc");
        }

        let wayland_display = session
            .wayland_display
            .clone()
            .unwrap_or_else(|| "wayland-0".to_string());

        Ok(Self {
            wayland_display,
            password_file: config.vnc_password_file.clone(),
            child: tokio::sync::Mutex::new(None),
        })
    }
}

#[async_trait::async_trait]
impl VncBackend for WayvncBackend {
    fn local_port(&self) -> u16 {
        LOCAL_PORT
    }

    async fn start(&self) -> Result<()> {
        let mut cmd = Command::new("wayvnc");
        cmd.arg("-o")
            .arg(self.local_port().to_string())
            .arg("-D")
            .env("WAYLAND_DISPLAY", &self.wayland_display);

        if let Some(pw) = &self.password_file {
            if std::path::Path::new(pw).exists() {
                cmd.arg("-P").arg(pw);
            }
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let child = cmd.spawn().context("spawn wayvnc")?;
        *self.child.lock().await = Some(child);
        info!(wayland = %self.wayland_display, "started wayvnc");

        for _ in 0..20 {
            sleep(Duration::from_millis(300)).await;
            if self.health_check().await? {
                return Ok(());
            }
        }
        bail!("wayvnc health check failed")
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

fn which_wayvnc() -> Option<String> {
    std::process::Command::new("which")
        .arg("wayvnc")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| "wayvnc".into())
}
