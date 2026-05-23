# Ubuntu Remote Control (URC)

Session-faithful remote desktop for Linux: attaches to your **real** X11 or Wayland session (GPU-accelerated), encrypts traffic by default, and connects through NAT via a self-hosted coordinator or optional Tailscale.

## Components

| Binary | Role |
|--------|------|
| `urc-agent` | Runs on Linux hosts — detects session, starts VNC backend, TLS tunnel, coordinator registration |
| `urc-coordinator` | Rendezvous + relay (run on a VPS) |
| `urc-client` | Connect from Linux/macOS — relays VNC and launches `vncviewer` |
| `urc-files` | HTTP file API (embedded in agent) |

## Quick start (development)

```bash
# Build
cargo build --release

# Terminal 1 — coordinator
./target/release/urc-coordinator --shared-secret mytoken

# Terminal 2 — agent (on machine with logged-in GUI)
sudo install -d /etc/urc
./target/release/urc-agent --init-config | sudo tee /etc/urc/agent.toml
# Edit host_id, token, coordinator_url → ws://YOUR_VPS:21150/ws/agent

# Terminal 3 — client
./target/release/urc-client --token mytoken connect my-hostname
```

## Install (production)

```bash
curl -fsSL https://raw.githubusercontent.com/ubuntu-remote-control/ubuntu-remote-control/main/packaging/install.sh | \
  sudo bash -s -- --profile minimal --token YOUR_SECRET
```

See [docs/headless-server.md](docs/headless-server.md) for GPU headless setups.

## VNC backends

- **X11:** `x0vncserver` (TigerVNC) on active display
- **GNOME Wayland:** `gnome-remote-desktop` via `grdctl`
- **wlroots:** `wayvnc` (Sway, Hyprland, …)

## Reliability

URC is designed to stay reachable while you travel:

- **systemd** `Restart=always` + no start-limit lockout
- **In-process supervisor** restarts VNC/coordinator if they die
- **10-minute health timer** runs `urc-health-check.sh` (restart if stuck)
- **Login path unit** re-triggers agent when a graphical session appears after reboot

See [docs/service-health.md](docs/service-health.md) for details and recovery tests.

```bash
urc-agent status    # JSON health
urc-agent health    # exit 0 if OK (used by watchdog)
systemctl status urc-agent-health.timer
```

## Security

- VNC binds **localhost only**; `urc-agent` exposes **TLS** on port 15900 by default
- Use `--insecure` only on trusted LANs
- Coordinator requires matching `--token` / `shared_secret`

## License

MIT (application code). VNC backends (TigerVNC, GNOME) are GPL — invoked as separate system packages.
