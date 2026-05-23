//! Background tunnel session bookkeeping.
//!
//! Each backgrounded `urc connect` writes a JSON descriptor under
//! `~/.cache/urc/sessions/<host>.json`. `urc sessions` reads the dir,
//! `urc kill TARGET` matches by host or pid, sends SIGTERM, removes the file.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub host: String,
    pub ip: String,
    pub pid: u32,
    pub local_port: u16,
    pub url: String,
    pub started_at_unix: u64,
}

pub fn sessions_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))
        })
        .context("HOME not set")?;
    let dir = base.join("urc").join("sessions");
    fs::create_dir_all(&dir).ok();
    Ok(dir)
}

fn session_path(host: &str) -> Result<PathBuf> {
    Ok(sessions_dir()?.join(format!("{host}.json")))
}

pub fn write(session: &Session) -> Result<()> {
    let path = session_path(&session.host)?;
    let json = serde_json::to_string_pretty(session)?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn remove(host: &str) -> Result<()> {
    let path = session_path(host)?;
    let _ = fs::remove_file(path);
    Ok(())
}

pub fn list() -> Result<Vec<Session>> {
    let dir = sessions_dir()?;
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(&dir)?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&p) {
            if let Ok(s) = serde_json::from_str::<Session>(&text) {
                out.push(s);
            }
        }
    }
    // Prune dead PIDs so `urc sessions` doesn't accumulate stale entries.
    out.retain(|s| {
        if pid_alive(s.pid) {
            true
        } else {
            let _ = remove(&s.host);
            false
        }
    });
    out.sort_by(|a, b| a.host.cmp(&b.host));
    Ok(out)
}

pub fn find(target: &str) -> Result<Option<Session>> {
    let all = list()?;
    if let Ok(pid) = target.parse::<u32>() {
        return Ok(all.into_iter().find(|s| s.pid == pid));
    }
    Ok(all.into_iter().find(|s| s.host == target))
}

pub fn kill(target: &str) -> Result<Session> {
    let Some(session) = find(target)? else {
        anyhow::bail!("no urc session matching '{target}' (try: urc sessions)");
    };
    send_sigterm(session.pid)?;
    remove(&session.host)?;
    Ok(session)
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn human_age(unix: u64) -> String {
    let dt = now().saturating_sub(unix);
    if dt < 60 {
        format!("{dt}s")
    } else if dt < 3600 {
        format!("{}m", dt / 60)
    } else if dt < 86400 {
        format!("{}h", dt / 3600)
    } else {
        format!("{}d", dt / 86400)
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists() || kill_check(pid)
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    true
}

#[cfg(unix)]
fn kill_check(pid: u32) -> bool {
    // Signal 0 is a "is this PID alive and signalable" probe on POSIX systems.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(unix)]
fn send_sigterm(pid: u32) -> Result<()> {
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if rc != 0 {
        anyhow::bail!("kill({pid}) failed: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn send_sigterm(_pid: u32) -> Result<()> {
    anyhow::bail!("session kill not supported on this platform");
}
