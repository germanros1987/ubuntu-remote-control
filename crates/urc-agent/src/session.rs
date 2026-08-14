//! Detect active graphical sessions via loginctl and environment.

use anyhow::{bail, Context, Result};
use std::os::unix::fs::MetadataExt;
use std::process::Command;
use urc_common::BackendPreference;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    X11,
    GnomeWayland,
    WlrootsWayland,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub backend_kind: BackendKind,
    pub display: Option<String>,
    pub wayland_display: Option<String>,
    pub xauthority: Option<String>,
    // Populated but not yet consumed by any caller. See follow-up task.
    #[allow(dead_code)]
    pub uid: u32,
    pub username: String,
    #[allow(dead_code)]
    pub desktop: Option<String>,
}

pub struct SessionDetector;

impl SessionDetector {
    pub fn detect(preference: BackendPreference) -> Result<SessionInfo> {
        if let Ok(info) = Self::from_loginctl() {
            return Self::apply_preference(info, preference);
        }
        Self::from_env(preference)
    }

    fn apply_preference(info: SessionInfo, preference: BackendPreference) -> Result<SessionInfo> {
        match preference {
            BackendPreference::Auto => Ok(info),
            BackendPreference::X11 => {
                if info.backend_kind != BackendKind::X11 {
                    bail!(
                        "backend preference x11 but active session is {:?}. \
                         Log in on Xorg or set backend = \"auto\".",
                        info.backend_kind
                    );
                }
                Ok(info)
            }
            BackendPreference::Gnome => {
                if info.backend_kind != BackendKind::GnomeWayland {
                    bail!("backend preference gnome but session is not GNOME Wayland");
                }
                Ok(info)
            }
            BackendPreference::Wayvnc => {
                if info.backend_kind != BackendKind::WlrootsWayland {
                    bail!("backend preference wayvnc but session is not wlroots Wayland");
                }
                Ok(info)
            }
        }
    }

    fn from_loginctl() -> Result<SessionInfo> {
        let output = Command::new("loginctl")
            .args(["list-sessions", "--no-legend"])
            .output()
            .context("loginctl not available")?;

        if !output.status.success() {
            bail!("loginctl failed");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }
            let session_id = parts[0];
            let uid: u32 = parts[1].parse().unwrap_or(0);
            if uid == 0 {
                continue;
            }

            let show = Command::new("loginctl")
                .args([
                    "show-session",
                    session_id,
                    "-p",
                    "Type",
                    "-p",
                    "Display",
                    "-p",
                    "Name",
                    "-p",
                    "Active",
                ])
                .output()?;

            if !show.status.success() {
                continue;
            }

            let show_text = String::from_utf8_lossy(&show.stdout);
            let mut session_type = String::new();
            let mut display = None;
            let mut username = None;
            let mut active = false;

            for l in show_text.lines() {
                if let Some(v) = l.strip_prefix("Type=") {
                    session_type = v.to_string();
                } else if let Some(v) = l.strip_prefix("Display=") {
                    if !v.is_empty() {
                        display = Some(v.to_string());
                    }
                } else if let Some(v) = l.strip_prefix("Name=") {
                    username = Some(v.to_string());
                } else if l == "Active=yes" {
                    active = true;
                }
            }

            if !active {
                continue;
            }

            let username = username.unwrap_or_else(|| format!("uid{uid}"));
            let xauth_glob = format!("/run/user/{uid}/.mutter-Xwaylandauth.*");

            // Resolve XAUTHORITY for X11
            let xauthority = if session_type == "x11" {
                resolve_xauthority(uid).or(Some(format!("/run/user/{uid}/gdm/Xauthority")))
            } else {
                glob_xauthority(&xauth_glob).or(resolve_xauthority(uid))
            };

            let display = if session_type == "x11" {
                display
                    .or_else(|| display_from_who(&username))
                    .or_else(|| display_from_x11_unix(uid))
                    .or_else(|| user_display_env(&username))
            } else {
                display
            };

            let desktop = detect_desktop(&username);
            let backend_kind = match session_type.as_str() {
                "x11" => BackendKind::X11,
                "wayland" => match desktop.as_deref() {
                    Some("gnome") | Some("ubuntu") => BackendKind::GnomeWayland,
                    Some("sway") | Some("hyprland") | Some("wayfire") => {
                        BackendKind::WlrootsWayland
                    }
                    _ => {
                        if desktop.is_some() {
                            bail!(
                                "Wayland session detected (desktop={:?}) but desktop environment is not supported. \
                                 Supported: GNOME (gnome-remote-desktop) or wlroots (wayvnc). \
                                 See docs/headless-server.md",
                                desktop
                            );
                        }
                        bail!(
                            "Wayland session without recognized desktop. \
                             Install GNOME or use Xorg (WaylandEnable=false in gdm custom.conf)."
                        );
                    }
                },
                other => bail!("unsupported session type: {other}"),
            };

            return Ok(SessionInfo {
                backend_kind,
                display: display.clone(),
                wayland_display: if session_type == "wayland" {
                    Some("wayland-0".to_string())
                } else {
                    display
                },
                xauthority,
                uid,
                username,
                desktop,
            });
        }

        bail!(
            "no active graphical session found. \
             On a headless server install the desktop profile: \
             install.sh --profile desktop --gpu auto. \
             Or log in locally to start a session."
        )
    }

    fn from_env(preference: BackendPreference) -> Result<SessionInfo> {
        let display = std::env::var("DISPLAY").ok();
        let wayland = std::env::var("WAYLAND_DISPLAY").ok();
        let uid = users_uid().unwrap_or(1000);
        let username = std::env::var("USER").unwrap_or_else(|_| format!("uid{uid}"));

        let info = if wayland.is_some() {
            let desktop = detect_desktop(&username);
            let kind = match desktop.as_deref() {
                Some("gnome") | Some("ubuntu") => BackendKind::GnomeWayland,
                Some("sway") | Some("hyprland") => BackendKind::WlrootsWayland,
                _ => bail!("Wayland without supported DE in current environment"),
            };
            SessionInfo {
                backend_kind: kind,
                display: None,
                wayland_display: wayland,
                xauthority: std::env::var("XAUTHORITY").ok(),
                uid,
                username,
                desktop,
            }
        } else if display.is_some() {
            SessionInfo {
                backend_kind: BackendKind::X11,
                display,
                wayland_display: None,
                xauthority: std::env::var("XAUTHORITY").ok(),
                uid,
                username: username.clone(),
                desktop: detect_desktop(&username),
            }
        } else {
            bail!("no DISPLAY or WAYLAND_DISPLAY and loginctl found no session")
        };

        Self::apply_preference(info, preference)
    }
}

/// Active TTY display from `who` (e.g. `:1`). loginctl often leaves Display= empty on Xorg.
fn display_from_who(username: &str) -> Option<String> {
    let output = Command::new("who").output().ok()?;
    if !output.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        if parts.next()? != username {
            continue;
        }
        let disp = parts.next()?;
        if disp.starts_with(':') && disp.chars().all(|c| c == ':' || c.is_ascii_digit()) {
            return Some(disp.to_string());
        }
    }
    None
}

/// Match /tmp/.X11-unix/X{n} sockets owned by the session uid.
fn display_from_x11_unix(uid: u32) -> Option<String> {
    let dir = std::path::Path::new("/tmp/.X11-unix");
    let mut found = Vec::new();
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(num) = name.strip_prefix('X').and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let meta = entry.metadata().ok()?;
        if meta.uid() == uid {
            found.push(num);
        }
    }
    found.sort_unstable();
    found.last().map(|n| format!(":{n}"))
}

fn user_display_env(username: &str) -> Option<String> {
    let output = Command::new("runuser")
        .args([
            "-u",
            username,
            "--",
            "bash",
            "-lc",
            "printf '%s' \"${DISPLAY}\"",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() || !s.starts_with(':') {
        None
    } else {
        Some(s)
    }
}

fn users_uid() -> Option<u32> {
    std::env::var("UID").ok().and_then(|s| s.parse().ok())
}

fn resolve_xauthority(uid: u32) -> Option<String> {
    let path = format!("/run/user/{uid}/gdm/Xauthority");
    if std::path::Path::new(&path).exists() {
        return Some(path);
    }
    let home = format!("/run/user/{uid}");
    std::fs::read_dir(&home).ok().and_then(|entries| {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(".mutter-Xwaylandauth") || name == "Xauthority" {
                return Some(e.path().to_string_lossy().to_string());
            }
        }
        None
    })
}

fn glob_xauthority(pattern: &str) -> Option<String> {
    let base = pattern.rsplit_once('/').map(|(b, _)| b)?;
    let prefix = pattern.rsplit_once('*').map(|(_, p)| p)?;
    std::fs::read_dir(base).ok().and_then(|entries| {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(prefix.trim_start_matches('.')) || name.contains("Xwaylandauth") {
                return Some(e.path().to_string_lossy().to_string());
            }
        }
        None
    })
}

fn detect_desktop(username: &str) -> Option<String> {
    let output = Command::new("runuser")
        .args([
            "-u",
            username,
            "--",
            "bash",
            "-lc",
            "echo $XDG_CURRENT_DESKTOP",
        ])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();
    if s.is_empty() {
        return None;
    }
    Some(s.split(':').next().unwrap_or(&s).to_string())
}
