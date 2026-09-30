//! Suppress server-generated repeat while the X11 backend owns a lifetime pipe.

use anyhow::{ensure, Context, Result};
use std::io::Write;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xinput::ConnectionExt as _;
use x11rb::protocol::xkb::ConnectionExt as _;
use x11rb::protocol::{xinput, xkb};

/// Runs as the desktop user with its DISPLAY/XAUTHORITY. Stdin is a lifetime
/// pipe owned by the backend: EOF also cleans up if the agent dies unexpectedly.
pub async fn run() -> Result<()> {
    let (conn, _) = x11rb::connect(None).context("connect keyboard repeat guard to X11")?;
    let supported = conn.xkb_use_extension(1, 0)?.reply()?.supported;
    ensure!(
        supported,
        "XKB is required for automatic keyboard repeat restoration"
    );
    let keyboard = u16::from(xkb::ID::USE_CORE_KBD);
    let core_id = u16::from(conn.xkb_get_controls(keyboard)?.reply()?.device_id);
    let repeat = xkb::BoolCtrl::REPEAT_KEYS;
    conn.xinput_xi_query_version(2, 0)?.reply()?;
    let devices = conn.xinput_xi_query_device(xinput::Device::ALL)?.reply()?;
    // Setting core controls also changes attached keyboards. Auto-reset is per
    // device, so preserve each keyboard separately (including the XTEST slave).
    let mut originals = Vec::new();
    for device in devices.infos.iter().filter(|device| {
        device.deviceid == core_id
            || (device.type_ == xinput::DeviceType::SLAVE_KEYBOARD && device.attachment == core_id)
    }) {
        let original = conn
            .xkb_get_controls(device.deviceid)?
            .reply()?
            .enabled_controls;
        originals.push((device.deviceid, original & repeat));
        let reset = conn
            .xkb_per_client_flags(
                device.deviceid,
                xkb::PerClientFlag::AUTO_RESET_CONTROLS,
                xkb::PerClientFlag::AUTO_RESET_CONTROLS,
                repeat,
                repeat,
                original & repeat,
            )?
            .reply()?;
        ensure!(
            reset
                .value
                .contains(xkb::PerClientFlag::AUTO_RESET_CONTROLS),
            "XKB automatic restoration is unavailable"
        );
    }
    originals.sort_by_key(|&(id, _)| id != core_id);
    let guard = RepeatGuard { conn, originals };
    guard.set_repeat(keyboard, xkb::BoolCtrl::default())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let result = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        let _ = closed_tx.send(result);
    });
    println!("ready");
    std::io::stdout().flush()?;
    tokio::select! {
        result = closed_rx => { result??; }
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
    // Explicit requests also restore X11's keyboard feedback, which older X
    // servers fail to update when applying XKB auto-reset controls alone.
    drop(guard);
    Ok(())
}

struct RepeatGuard {
    conn: x11rb::rust_connection::RustConnection,
    originals: Vec<(u16, xkb::BoolCtrl)>,
}

impl RepeatGuard {
    fn set_repeat(&self, keyboard: u16, enabled: xkb::BoolCtrl) -> Result<()> {
        self.conn
            .send_trait_request_without_reply(xkb::SetControlsRequest {
                device_spec: keyboard,
                affect_enabled_controls: xkb::BoolCtrl::REPEAT_KEYS,
                enabled_controls: enabled,
                change_controls: xkb::Control::CONTROLS_ENABLED,
                ..Default::default()
            })?
            .check()?;
        Ok(())
    }
}

impl Drop for RepeatGuard {
    fn drop(&mut self) {
        // Restore the master first, then its slaves, preserving per-device
        // differences in the original state.
        for &(keyboard, enabled) in &self.originals {
            let _ = self.set_repeat(keyboard, enabled);
        }
        let _ = self.conn.flush();
    }
}
