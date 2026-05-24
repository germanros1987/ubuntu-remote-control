//! `urc-agent share` — print a QR code + `urc://` deep link that pairs the
//! Android client with this machine.
//!
//! The payload carries only routing hints (tailnet IPv4, MagicDNS name, port,
//! display name). Tailnet membership is the authentication boundary, so there
//! are deliberately no secrets in the link.

use anyhow::{Context, Result};
use qrcode::render::unicode;
use qrcode::QrCode;
use urc_common::tailscale;
use urc_common::DEFAULT_WEB_TLS_PORT;

/// Build the `urc://connect?...` deep link for this machine and render it as a
/// terminal QR code plus the raw string.
pub fn run_share() -> Result<()> {
    let json = tailscale::status_json()?;
    tailscale::ensure_local_running(&json)?;

    let host = tailscale::self_ipv4(&json)
        .context("this machine has no tailnet IPv4 — run: tailscale up")?;
    let magicdns = tailscale::self_dns_name(&json)
        .context("this machine has no MagicDNS name — is MagicDNS enabled on your tailnet?")?;
    let name = local_hostname();

    let uri = build_uri(&host, &magicdns, DEFAULT_WEB_TLS_PORT, &name);

    let code = QrCode::new(uri.as_bytes()).context("encode pairing QR code")?;
    let qr = code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build();

    println!("\nScan with the URC Android app to pair with \"{name}\":\n");
    println!("{qr}");
    println!("{uri}\n");
    Ok(())
}

/// Assemble the deep link with URL-encoded query values.
fn build_uri(host: &str, magicdns: &str, port: u16, name: &str) -> String {
    format!(
        "urc://connect?host={}&magicdns={}&port={}&name={}",
        encode(host),
        encode(magicdns),
        port,
        encode(name),
    )
}

/// Short hostname of this machine — `/etc/hostname` is the canonical source on
/// Linux and stable across init systems (mirrors urc-files::host_info).
fn local_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .arg("-s")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "remote".to_string())
}

/// Percent-encode a query value. Encodes everything outside the unreserved set
/// (RFC 3986 `2.3`), which is more than enough for hostnames and IPv4s.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_leaves_unreserved_chars() {
        assert_eq!(encode("my-pc.tail-scale.ts.net"), "my-pc.tail-scale.ts.net");
        assert_eq!(encode("100.64.0.1"), "100.64.0.1");
    }

    #[test]
    fn encode_escapes_spaces_and_specials() {
        assert_eq!(encode("My Laptop"), "My%20Laptop");
        assert_eq!(encode("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn build_uri_assembles_query() {
        let uri = build_uri("100.64.0.1", "host.ts.net", 15901, "My PC");
        assert_eq!(
            uri,
            "urc://connect?host=100.64.0.1&magicdns=host.ts.net&port=15901&name=My%20PC"
        );
    }
}
