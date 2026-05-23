//! Health and status reporting for watchdog scripts.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use urc_common::AgentConfig;

pub const STATUS_PATH: &str = "/run/urc/status.json";
const MAX_STATUS_AGE_SECS: u64 = 90;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub healthy: bool,
    pub session_detected: bool,
    pub vnc_port_open: bool,
    pub coordinator_connected: bool,
    pub files_port_open: bool,
    pub backend: Option<String>,
    pub display: Option<String>,
    pub last_error: Option<String>,
    pub updated_at_unix: u64,
    pub supervisor_cycle: u64,
}

impl AgentStatus {
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json).context("write status file")?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).context("read status file")?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn is_fresh(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.updated_at_unix) <= MAX_STATUS_AGE_SECS
    }
}

pub async fn tcp_open(port: u16) -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_ok()
}

pub async fn probe_health(config: &AgentConfig) -> AgentStatus {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let session_detected = crate::session::SessionDetector::detect(config.backend).is_ok();
    let vnc_port_open = tcp_open(5900).await;
    let files_port_open = tcp_open(urc_common::DEFAULT_FILES_PORT).await;

    let needs_coordinator = !config.coordinator_url.trim().is_empty();
    let status_path = PathBuf::from(STATUS_PATH);
    let coordinator_connected = if !needs_coordinator {
        true
    } else if let Ok(file) = AgentStatus::read(&status_path) {
        file.is_fresh() && file.coordinator_connected
    } else {
        false
    };

    let healthy = session_detected && vnc_port_open && coordinator_connected;

    AgentStatus {
        healthy,
        session_detected,
        vnc_port_open,
        coordinator_connected,
        files_port_open,
        backend: None,
        display: None,
        last_error: if healthy {
            None
        } else {
            Some(build_error_summary(
                session_detected,
                vnc_port_open,
                coordinator_connected,
            ))
        },
        updated_at_unix: now,
        supervisor_cycle: 0,
    }
}

fn build_error_summary(session: bool, vnc: bool, coord: bool) -> String {
    let mut parts = Vec::new();
    if !session {
        parts.push("no graphical session");
    }
    if !vnc {
        parts.push("VNC port closed");
    }
    if !coord {
        parts.push("coordinator disconnected");
    }
    parts.join("; ")
}

pub async fn run_health_check(config: &AgentConfig) -> Result<()> {
    let path = PathBuf::from(STATUS_PATH);
    if path.exists() {
        if let Ok(status) = AgentStatus::read(&path) {
            if status.is_fresh() {
                if status.healthy {
                    return Ok(());
                }
                anyhow::bail!(
                    "unhealthy: {}",
                    status.last_error.unwrap_or_else(|| "unknown".into())
                );
            }
        }
    }

    let probe = probe_health(config).await;
    if probe.healthy {
        Ok(())
    } else {
        anyhow::bail!(
            "unhealthy: {}",
            probe.last_error.unwrap_or_else(|| "probe failed".into())
        )
    }
}

pub async fn run_status_cmd(config: &AgentConfig) -> Result<()> {
    let path = PathBuf::from(STATUS_PATH);
    let status = if path.exists() {
        match AgentStatus::read(&path) {
            Ok(s) if s.is_fresh() => s,
            _ => probe_health(config).await,
        }
    } else {
        probe_health(config).await
    };
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}
