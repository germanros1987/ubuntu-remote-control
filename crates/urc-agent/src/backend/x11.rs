//! X11 backend using TigerVNC screen scraping (x0tigervncserver).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
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
    username: String,
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
            username: session.username.clone(),
            xauthority: session.xauthority.clone(),
            password_file,
            child: tokio::sync::Mutex::new(None),
        })
    }

    fn resolved_xauthority(&self) -> Option<String> {
        self.xauthority.as_ref().and_then(|p| {
            if Path::new(p).exists() {
                Some(p.clone())
            } else if p.contains('*') {
                // glob already resolved in session layer when possible
                None
            } else {
                None
            }
        })
    }
}

#[async_trait::async_trait]
impl VncBackend for X11Backend {
    fn local_port(&self) -> u16 {
        LOCAL_PORT
    }

    async fn start(&self) -> Result<()> {
        let vnc_bin = super::vnc_bin::screen_share_vnc_server()?;

        // Must run as the logged-in desktop user (root cannot scrape the user's X display).
        let mut cmd = Command::new("runuser");
        cmd.args(["-u", &self.username, "--", "env"]);
        cmd.arg(format!("DISPLAY={}", self.display));
        if let Some(xauth) = self.resolved_xauthority() {
            cmd.arg(format!("XAUTHORITY={xauth}"));
        }
        cmd.arg(&vnc_bin);
        cmd.args([
            "-display",
            &self.display,
            "-rfbport",
            &self.local_port().to_string(),
            "-localhost",
            "yes",
            "-UseSHM",
            "1",
            "-FrameRate",
            "60",
            "-SecurityTypes",
            "VncAuth",
        ]);

        if self.password_file.exists() {
            cmd.arg("-PasswordFile").arg(&self.password_file);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let child = cmd.spawn().context("spawn screen VNC server as desktop user")?;
        info!(
            user = %self.username,
            display = %self.display,
            port = LOCAL_PORT,
            bin = %vnc_bin,
            "started screen VNC"
        );

        *self.child.lock().await = Some(child);

        for _ in 0..40 {
            sleep(Duration::from_millis(250)).await;
            if self.health_check().await.unwrap_or(false) {
                return Ok(());
            }
        }

        bail!(
            "screen VNC server failed on port {LOCAL_PORT} (user={}, display={}). \
             Check: journalctl -u urc-agent -e",
            self.username, self.display
        )
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
