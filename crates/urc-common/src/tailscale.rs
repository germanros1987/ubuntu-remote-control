//! Tailscale peer discovery via `tailscale status --json`.

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct TailscalePeer {
    pub name: String,
    pub dns_name: String,
    pub ipv4: String,
    pub online: bool,
}

/// Resolve the `tailscale` CLI (macOS App Store builds are not always on PATH).
pub fn tailscale_bin() -> Result<PathBuf> {
    for key in ["URC_TAILSCALE_BIN", "TAILSCALE_BIN"] {
        if let Ok(p) = std::env::var(key) {
            let path = PathBuf::from(p.trim());
            if path.as_os_str().is_empty() {
                continue;
            }
            if let Some(resolved) = resolve_tailscale_exec(&path) {
                return Ok(resolved);
            }
            anyhow::bail!("{key} is set but not executable: {}", path.display());
        }
    }

    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':').filter(|d| !d.is_empty()) {
            let candidate = Path::new(dir).join("tailscale");
            if let Some(resolved) = resolve_tailscale_exec(&candidate) {
                return Ok(resolved);
            }
        }
    }

    for candidate in default_candidates() {
        if let Some(resolved) = resolve_tailscale_exec(&candidate) {
            return Ok(resolved);
        }
    }

    #[cfg(target_os = "macos")]
    let hint = "Install Tailscale from https://tailscale.com/download/mac, sign in via the \
menu bar app, then run the URC installer again (it adds a safe CLI wrapper — do not symlink).";
    #[cfg(not(target_os = "macos"))]
    let hint = "Install Tailscale: https://tailscale.com/download";

    anyhow::bail!(
        "tailscale CLI not found. {hint}\n\
         Or set URC_TAILSCALE_BIN to the Tailscale binary inside Tailscale.app."
    );
}

/// Path suitable for `Command::new` — never a symlink to the macOS .app binary.
fn resolve_tailscale_exec(path: &Path) -> Option<PathBuf> {
    if !is_executable(path) {
        return None;
    }
    Some(normalize_tailscale_exec(path))
}

fn normalize_tailscale_exec(path: &Path) -> PathBuf {
    if path.is_symlink() {
        if let Ok(target) = std::fs::read_link(path) {
            let resolved = if target.is_absolute() {
                target
            } else {
                path.parent()
                    .unwrap_or_else(|| Path::new("/"))
                    .join(target)
            };
            if resolved.is_file() {
                return resolved;
            }
        }
        if let Ok(canonical) = path.canonicalize() {
            return canonical;
        }
    }
    path.to_path_buf()
}

fn default_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![
        PathBuf::from("/usr/local/bin/tailscale"),
        PathBuf::from("/opt/homebrew/bin/tailscale"),
        PathBuf::from("/usr/bin/tailscale"),
    ];
    #[cfg(target_os = "macos")]
    {
        out.push(PathBuf::from(
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ));
    }
    out
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Run `tailscale status --json` and parse stdout.
pub fn status_json() -> Result<Value> {
    let bin = normalize_tailscale_exec(&tailscale_bin()?);
    let output = Command::new(&bin)
        .args(["status", "--json"])
        .output()
        .with_context(|| {
            format!(
                "run {} status — is Tailscale installed and logged in?",
                bin.display()
            )
        })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("tailscale status failed: {err}");
    }
    let json: Value =
        serde_json::from_slice(&output.stdout).context("parse tailscale status --json")?;
    Ok(json)
}

/// IPv4 address of this machine on the tailnet.
pub fn self_ipv4(json: &Value) -> Option<String> {
    peer_ipv4(json.get("Self")?)
}

/// MagicDNS name of this machine, with any trailing dot stripped. The `Self`
/// object carries `DNSName` just like peers do (see `peer_from_value`).
pub fn self_dns_name(json: &Value) -> Option<String> {
    let dns = json
        .get("Self")?
        .get("DNSName")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty())?;
    Some(dns)
}

/// Local Tailscale daemon state — one of NoState / NeedsLogin / NeedsMachineAuth /
/// Stopped / Starting / Running. Returns the raw string so callers can produce a
/// targeted error message; missing field is treated as Unknown.
pub fn local_backend_state(json: &Value) -> &str {
    json.get("BackendState")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
}

/// Whether this machine's tailnet membership looks healthy enough to reach peers.
pub fn ensure_local_running(json: &Value) -> Result<()> {
    let state = local_backend_state(json);
    if state == "Running" {
        // Check Self.Online too — daemon up but node may be flagged offline by control plane.
        let self_online = json
            .get("Self")
            .and_then(|s| s.get("Online"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !self_online {
            anyhow::bail!(
                "Tailscale is running locally but this machine is offline on the tailnet \
                 (control plane has not seen it).\n\
                 Try: tailscale down && tailscale up"
            );
        }
        return Ok(());
    }
    let hint = match state {
        "NeedsLogin" | "NoState" => "Run: tailscale up",
        "NeedsMachineAuth" => "Approve this machine in the Tailscale admin console.",
        "Stopped" => "Run: tailscale up",
        "Starting" => "Tailscale is still connecting — retry in a few seconds.",
        _ => "Run: tailscale up",
    };
    anyhow::bail!(
        "Tailscale is not connected on this machine (BackendState={state}).\n{hint}"
    )
}

/// All remote peers in the tailnet (excludes this machine).
pub fn list_peers(json: &Value) -> Vec<TailscalePeer> {
    let Some(peers) = json.get("Peer").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (_id, peer) in peers {
        if let Some(p) = peer_from_value(peer) {
            out.push(p);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Find a peer by short name, hostname, or MagicDNS name (case-insensitive).
pub fn resolve_peer<'a>(peers: &'a [TailscalePeer], query: &str) -> Option<&'a TailscalePeer> {
    let q = query.trim().trim_end_matches('.').to_lowercase();
    if q.is_empty() {
        return None;
    }
    peers.iter().find(|p| {
        p.name.to_lowercase() == q
            || p.dns_name.to_lowercase() == q
            || short_name(&p.dns_name).to_lowercase() == q
            || p.dns_name
                .trim_end_matches('.')
                .to_lowercase()
                .starts_with(&format!("{q}."))
    })
}

pub fn short_name(dns_name: &str) -> String {
    dns_name
        .trim_end_matches('.')
        .split('.')
        .next()
        .unwrap_or(dns_name)
        .to_string()
}

fn peer_from_value(peer: &Value) -> Option<TailscalePeer> {
    let ipv4 = peer_ipv4(peer)?;
    let dns = peer
        .get("DNSName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let hostname = peer
        .get("HostName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| short_name(&dns));
    let online = peer
        .get("Online")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(TailscalePeer {
        name: hostname,
        dns_name: dns,
        ipv4,
        online,
    })
}

fn peer_ipv4(peer: &Value) -> Option<String> {
    peer.get("TailscaleIPs")
        .and_then(|v| v.as_array())
        .and_then(|ips| {
            ips.iter().find_map(|ip| {
                let s = ip.as_str()?;
                if s.contains(':') {
                    None
                } else {
                    Some(s.to_string())
                }
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_strips_tailnet_suffix() {
        assert_eq!(short_name("my-pc.tailnet.ts.net."), "my-pc");
    }
}
