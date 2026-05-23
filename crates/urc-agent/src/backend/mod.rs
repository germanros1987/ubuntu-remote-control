//! VNC backend plugins: X11 (x0vncserver), GNOME (grd), wlroots (wayvnc).

mod gnome;
mod vnc_bin;
mod wayvnc;
mod x11;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

use crate::session::{BackendKind, SessionInfo};
use urc_common::AgentConfig;

pub use x11::X11Backend;

#[async_trait::async_trait]
pub trait VncBackend: Send + Sync {
    fn local_port(&self) -> u16;
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn health_check(&self) -> Result<bool>;
}

pub struct BackendManager {
    inner: Arc<Mutex<Box<dyn VncBackend>>>,
}

impl BackendManager {
    pub fn new(config: &AgentConfig, session: &SessionInfo) -> Result<Self> {
        let backend: Box<dyn VncBackend> = match session.backend_kind {
            BackendKind::X11 => Box::new(x11::X11Backend::new(config, session)?),
            BackendKind::GnomeWayland => Box::new(gnome::GnomeBackend::new(config, session)?),
            BackendKind::WlrootsWayland => Box::new(wayvnc::WayvncBackend::new(config, session)?),
        };
        Ok(Self {
            inner: Arc::new(Mutex::new(backend)),
        })
    }

    pub async fn start(&self) -> Result<u16> {
        let guard = self.inner.lock().await;
        guard.start().await?;
        let port = guard.local_port();
        info!(port, "VNC backend listening on localhost");
        Ok(port)
    }

    pub async fn stop(&self) {
        let guard = self.inner.lock().await;
        if let Err(e) = guard.stop().await {
            tracing::warn!(error = %e, "backend stop error");
        }
    }

    pub async fn health_check(&self) -> bool {
        let guard = self.inner.lock().await;
        guard.health_check().await.unwrap_or(false)
    }
}
