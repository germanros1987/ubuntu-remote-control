//! Tailscale peer discovery via `tailscale status --json`.

use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct TailscalePeer {
    pub name: String,
    pub dns_name: String,
    pub ipv4: String,
    pub online: bool,
}

/// Run `tailscale status --json` and parse stdout.
pub fn status_json() -> Result<Value> {
    let output = Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .context("run tailscale status — is Tailscale installed and logged in?")?;
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
