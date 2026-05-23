//! Locate TigerVNC screen-scraping server binaries across distro renames.

use anyhow::{bail, Result};
use std::process::Command;

/// Path to a binary that shares the active X11 display (not a virtual VNC desktop).
pub fn screen_share_vnc_server() -> Result<String> {
    for name in ["x0tigervncserver", "x0vncserver"] {
        if let Some(path) = which_bin(name) {
            return Ok(path);
        }
    }
    bail!(
        "screen VNC server not found (need x0tigervncserver).\n\
         On Ubuntu: sudo apt install tigervnc-scraping-server tigervnc-common\n\
         Or re-run: curl -fsSL …/install | sudo bash -s -- --role agent"
    )
}

fn which_bin(name: &str) -> Option<String> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}
