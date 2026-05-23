//! Optional Tailscale mesh direct-connect support.

use tracing::debug;

/// Detect Tailscale IPv4 address via `tailscale status --json`.
pub async fn detect_ip() -> Option<String> {
    let json = tokio::task::spawn_blocking(urc_common::tailscale::status_json)
        .await
        .ok()
        .and_then(|r| r.ok())?;
    urc_common::tailscale::self_ipv4(&json).or_else(|| {
        debug!("tailscale connected but no IPv4 yet");
        None
    })
}
