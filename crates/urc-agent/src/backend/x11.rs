//! X11 backend using TigerVNC screen scraping (x0tigervncserver).

use anyhow::{bail, Context, Result};
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncReadExt;
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
                None
            } else {
                None
            }
        })
    }

    async fn stop_stale_vnc(&self) {
        let port = self.local_port().to_string();
        let _ = Command::new("runuser")
            .args([
                "-u",
                &self.username,
                "--",
                "bash",
                "-lc",
                &format!(
                    "pkill -f 'x0tigervncserver.*-rfbport {port}' 2>/dev/null; \
                     pkill -f 'x0vncserver.*-rfbport {port}' 2>/dev/null; \
                     rm -f \"$HOME/.vnc\"/*.pid 2>/dev/null; true"
                ),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        sleep(Duration::from_millis(500)).await;
    }

    async fn display_accessible(&self) -> Result<()> {
        let mut cmd = Command::new("runuser");
        cmd.args(["-u", &self.username, "--", "env"]);
        cmd.arg(format!("DISPLAY={}", self.display));
        if let Some(xauth) = self.resolved_xauthority() {
            cmd.arg(format!("XAUTHORITY={xauth}"));
        }
        cmd.args(["xdpyinfo"]);
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        let out = cmd.output().await.context("xdpyinfo preflight")?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("Maximum number of clients reached") {
            bail!(
                "X display {} is saturated. Stop urc-agent, wait 60s, then start again.",
                self.display
            );
        }
        bail!(
            "cannot access X display {} for user {}: {}",
            self.display,
            self.username,
            err.trim()
        )
    }

    /// TigerVNC runs as the desktop user; password file must be readable by that user.
    async fn ensure_password_file(&self) -> Result<PathBuf> {
        let path = &self.password_file;
        if !path.exists() {
            return Ok(path.clone());
        }

        let readable = Command::new("runuser")
            .args([
                "-u",
                &self.username,
                "--",
                "test",
                "-r",
                path.to_str().context("password path")?,
            ])
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        if readable {
            return Ok(path.clone());
        }

        let uid_out = Command::new("id")
            .args(["-u", &self.username])
            .output()
            .await
            .context("id -u")?;
        let gid_out = Command::new("id")
            .args(["-g", &self.username])
            .output()
            .await
            .context("id -g")?;
        let uid: u32 = String::from_utf8_lossy(&uid_out.stdout)
            .trim()
            .parse()
            .context("parse uid")?;
        let gid: u32 = String::from_utf8_lossy(&gid_out.stdout)
            .trim()
            .parse()
            .context("parse gid")?;

        chown(path, Some(uid), Some(gid)).with_context(|| {
            format!(
                "chown {} for VNC password file {}",
                self.username,
                path.display()
            )
        })?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;

        info!(
            user = %self.username,
            path = %path.display(),
            "fixed VNC password file ownership"
        );
        Ok(path.clone())
    }

    async fn vnc_failure_reason(&self, mut child: Child) -> String {
        let mut stderr_buf = Vec::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_end(&mut stderr_buf).await;
        }
        child.kill().await.ok();
        let _ = child.wait().await;

        let mut reason = String::from_utf8_lossy(&stderr_buf).trim().to_string();
        if reason.is_empty() {
            let display_num = self.display.trim_start_matches(':');
            let host = Command::new("hostname")
                .arg("-s")
                .output()
                .await
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "localhost".to_string());
            let log_path = format!("/home/{}/.vnc/{}:{display_num}.log", self.username, host);
            if let Ok(log) = tokio::fs::read_to_string(&log_path).await {
                reason = log.lines().last().unwrap_or("").to_string();
            }
        }
        if reason.is_empty() {
            reason = "no stderr from VNC process".to_string();
        }
        reason
    }
}

#[async_trait::async_trait]
impl VncBackend for X11Backend {
    fn local_port(&self) -> u16 {
        LOCAL_PORT
    }

    async fn start(&self) -> Result<()> {
        let vnc_bin = super::vnc_bin::screen_share_vnc_server()?;

        self.stop_stale_vnc().await;
        self.display_accessible().await?;
        let password_file = self.ensure_password_file().await?;

        let mut cmd = Command::new("runuser");
        cmd.args(["-u", &self.username, "--", "env"]);
        cmd.arg(format!("DISPLAY={}", self.display));
        if let Some(xauth) = self.resolved_xauthority() {
            cmd.arg(format!("XAUTHORITY={xauth}"));
        }
        cmd.arg(&vnc_bin);
        // VNC listens on localhost only; Tailscale clients use TLS on :15900.
        // SecurityTypes None lets macOS Screen Sharing connect without a separate VNC password.
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
            "None",
            // Failed handshakes (e.g. Apple Screen Sharing protocol mismatch) otherwise
            // blacklist the source for minutes and reject future clients with
            // "Too many security failures". Tailnet + TLS already gate access.
            "-BlacklistThreshold",
            "1000000",
            "-BlacklistTimeout",
            "1",
        ]);
        let _ = password_file; // kept for API compat; auth is TLS + localhost bind

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .context("spawn screen VNC server as desktop user")?;
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

        let child = self.child.lock().await.take();
        let detail = if let Some(c) = child {
            self.vnc_failure_reason(c).await
        } else {
            "VNC process missing".to_string()
        };

        bail!(
            "screen VNC failed on port {LOCAL_PORT} (user={}, display={}): {detail}",
            self.username,
            self.display
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
