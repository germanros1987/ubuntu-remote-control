//! Optional Tailscale mesh direct-connect support.

use tracing::debug;

/// Detect Tailscale IPv4 address via `tailscale ip -4`.
pub async fn detect_ip() -> Option<String> {
    let output = tokio::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        debug!("tailscale not available or not connected");
        return None;
    }

    let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if ip.is_empty() || ip == "tailscale: command not found" {
        return None;
    }
    Some(ip)
}
