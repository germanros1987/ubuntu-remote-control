# URC Android Client — Usage and Pairing

This guide covers installing and using the URC Android client to connect to a shared Ubuntu desktop over Tailscale.

---

## Overview

The URC Android client is a native WebView app that mirrors the desktop/Mac client: an **on-device TLS-forward proxy** (binds loopback only) plus a full-screen UI that loads the agent's unified web interface. The app runs the same noVNC + files service stack as the desktop, so feature parity is high for phones with Tailscale connectivity.

**Prerequisites:**
- Official Tailscale Android app installed and signed in to your tailnet
- The remote Ubuntu PC must be running `urc-agent` with the `urc share` command available
- Android 7.0+ device (API 24+)

---

## Installation

### Get the APK

The URC Android client is distributed as `urc-android.apk`:

- **Official releases**: Download from [GitHub Releases](https://github.com/germanros1987/ubuntu-remote-control/releases) → `urc-android.apk`
- **Development builds**: Build from source (see `android/README.md` → Build)

### Sideload to your device

1. Transfer `urc-android.apk` to your Android device (USB, email, cloud storage, etc.)
2. Open a file manager and tap the APK to install
3. You may see a security prompt ("Unknown app"); tap **Install Anyway** or **Install**
4. On Android 12+, grant **Allow installation of apps from unknown sources** if prompted

Once installed, open the app from your launcher (icon: URC).

---

## Pairing a PC

### Option 1: Scan a QR code (recommended)

1. **On the remote PC**, run:
   ```bash
   urc share
   ```
   This prints a QR code to the terminal and the raw `urc://connect?…` link.

2. **In the URC Android app**, tap **Scan QR**:
   - Grant camera permission if prompted
   - Point the camera at the QR on the terminal
   - The app auto-detects the code and navigates to pair

3. Tap **Connect** to save the PC and start the session

### Option 2: Tap a deep link (if you have the `urc://` URL)

If someone sends you a `urc://connect?…` link (SMS, email, etc.), tap it on your Android device. The app opens automatically and navigates to the pairing screen. Tap **Connect** to save and connect.

### Option 3: Manual entry

If QR and deep links don't work:

1. **On the remote PC**, run `urc share` and note the **IPv4 address** and **port** (e.g., `100.80.198.81:15901`)
2. **In the URC Android app**, tap **Add Manually** and enter the Tailscale IPv4 + port
3. Tap **Connect**

---

## Connecting and using the desktop

1. **Tap a saved host** in the URC app home screen
2. The app connects via the loopback TLS proxy and loads the web UI in fullscreen
3. You'll see the remote desktop, files drawer, and soft-keyboard button:

   - **Remote desktop**: Pan and pinch to zoom; two-finger tap for right-click
   - **Files drawer** (swipe left edge or tap ☰): Upload single/multiple files or download them as ZIP
   - **Soft keyboard** (⌨ button): On-screen keyboard for text input
   - **Exit** (tap top-left X or back button): Close the session and return to host list

### File operations

- **Upload**: Tap the upload icon, select single or multiple files from your device, and they'll be written to the remote home directory (or configured `files_root`)
- **Download**: Navigate folders, tap a file, choose **Download**, and it saves via your device's DownloadManager to `Downloads/`
- **Download folder as ZIP**: Select a folder in the files drawer and tap **Download Folder** to get a ZIP archive

**Android WebView limitation**: Folder upload (the `webkitdirectory` attribute) is not supported; only individual files can be uploaded. This is a phase-2 improvement.

---

## Trust model and security

### Why it's safe

1. **Tailnet membership is the authentication boundary** — the Android app works only because you're already a member of the same Tailscale tailnet as the PC. No additional passwords or tokens are needed in the `urc://` QR payload.

2. **On-device proxy binds loopback only** — `LocalTlsProxy` binds `127.0.0.1` and accepts connections from the WebView on that interface only. No other app on your device or LAN can reach the tunnel.

3. **Tailscale encryption and identity** — TLS is **trust-all** (the agent's certificate is self-signed), but that's safe because every connection is cryptographically inside Tailscale's WireGuard tunnel. Confidentiality and authentication come from Tailscale, not TLS.

4. **CGNAT address guard** — The app enforces that the target IP address is inside `100.64.0.0/10` (Tailscale's CGNAT block). DNS resolution is **never** used for the dial; only the IPv4 literal from the QR or manual entry is trusted. This prevents DNS rebinding and TOCTOU attacks.

### QR payload (no secrets)

The `urc://connect?` link carries only routing hints:

```
urc://connect?host=100.80.198.81&magicdns=sa-grs.tail-abc-def.ts.net&port=15901&name=sa-grs
```

- **`host`**: Tailscale IPv4 (CGNAT range `100.64.0.0/10`)
- **`magicdns`**: DNS name (for display only; not used for dialing)
- **`port`**: Web TLS port (default `15901`)
- **`name`**: Short hostname (for display in the host list)

No secrets, tokens, or passwords are encoded.

---

## Known limitations (phase-2)

These features are compiled and follow Android SDK contracts, but have not been validated on a physical device in a full end-to-end setup (they require a real camera, Tailscale tailnet, and PC running the agent):

- **Live tunnel** — WebView ↔ proxy ↔ agent connectivity with real noVNC + files operations
- **Real QR scan** — Camera + ML Kit model download and scan recognition on first use
- **DownloadManager** — Writing `/api/download` and `/api/download-zip` outputs to device storage
- **File chooser** — `onShowFileChooser` round-trip with the upload form in the web UI
- **HTML5 fullscreen** — Immersive mode hiding Android status/nav bars
- **Foreground service survival** — Tunnel persistence across screen-off on ROMs with aggressive battery optimization
- **Folder upload** — Android WebView does not support the `webkitdirectory` attribute; only single/multi-file upload works

See `android/README.md` for architecture details and security model hardening.

---

## Troubleshooting

### "Tailscale is not available" or "Not signed in"

The URC app requires the official Tailscale Android app to be installed and active on your tailnet.

- **Install Tailscale**: Open Google Play Store, search "Tailscale", and install the official app by Tailscale Inc.
- **Sign in**: Launch Tailscale, tap "Sign in with your browser", and complete the sign-in flow
- **Check connectivity**: Run `tailscale status` on the remote PC; you should see your Android device in the peer list once signed in

### "Invalid QR code" or "Could not pair"

- Ensure the remote PC is running `urc-agent` and is healthy: `systemctl status urc-agent` on the PC
- Verify the PC is on the same Tailscale tailnet and your device is signed in
- Try manual entry: Get the IPv4 from `urc share` output and enter it manually in the app
- Check that the port is `15901` (not `5900`, which is internal to the PC)

### "Connection refused" or "Can't reach the host"

- The PC may not be sharing right now. On the remote PC, run `urc share` and check for errors
- Verify both devices are on the same Tailscale tailnet and can reach each other: `tailscale ping 100.x.y.z` from your phone (use Tailscale app → Devices to find IPs)
- Check the PC's `urc-agent` logs: `journalctl -u urc-agent -n 20`

### App crashes or "WebView error"

- Ensure you have the latest WebView: open Google Play Store, search "Android System WebView", and check for updates
- Try closing and reopening the app
- If crashes persist, check the logcat: `adb logcat | grep urc` and file an issue on GitHub

---

## See also

- **Desktop/Mac client**: `crates/urc-client/` (Rust TLS forward + screen sharing)
- **Agent internals**: `crates/urc-agent/` (VNC backend, TLS listener, file server)
- **Architecture**: `docs/HANDOFF.md` (overview, data paths, install, debugging)
- **Building the APK**: `android/README.md` (Gradle build, CI/release, code organization)
