# URC Handoff Report — May 2026

This document is for the next engineer picking up **ubuntu-remote-control** (URC). It summarizes product goals, architecture, deployment model, what was fixed in this session, and what is still broken or fragile.

**Repo:** https://github.com/germanros1987/ubuntu-remote-control  
**Owner:** German Ros  
**Primary test machines:** `sa-grs` (Ubuntu desktop, Tailscale `100.80.198.81`), MacBook Air (client)

---

## 1. Product goals

### What URC is supposed to do

1. **Turn-key remote desktop** for Ubuntu PCs the user owns — no manual VNC setup, no copying host IDs/tokens by hand for the default path.
2. **Tailscale-first** — machines on the same tailnet discover each other; `urc connect NAME` resolves NAME via `tailscale status` and connects.
3. **macOS client** uses **built-in Screen Sharing** (`open vnc://…`) — no TigerVNC install on Mac.
4. **Encrypted over the tailnet** — agent exposes VNC wrapped in TLS on port **15900**; client strips TLS locally and presents plain VNC to Screen Sharing.
5. **Optional VPS coordinator** — WebSocket relay for advanced setups (`coordinator_url` in config); most users should not need it.
6. **1-click install** via curl:
   - PC: `curl …/install | sudo bash -s -- --role agent -y`
   - Mac: `curl …/install | sudo bash -s -- --role client -y`

### What “done” looks like

| Check | Command / expectation |
|-------|---------------------|
| Agent running | `systemctl status urc-agent` → active |
| VNC on localhost | `ss -tlnp \| grep 5900` → `x0tigervnc` on `127.0.0.1:5900` |
| TLS on tailnet | `ss -tlnp \| grep 15900` → `urc-agent` on `0.0.0.0:15900` |
| Session detection | `journalctl -u urc-agent` → `display=Some(":1")` (not `:0` on multi-seat Xorg) |
| Mac connect | `urc connect sa-grs` → tunnel + Screen Sharing shows desktop |

---

## 2. Architecture

### Crates (Rust workspace)

| Crate | Role |
|-------|------|
| `urc-agent` | Runs on Ubuntu PC. Detects graphical session, starts VNC backend, TLS tunnel, optional coordinator client, optional files server. |
| `urc-client` | Runs on laptop. `urc hosts`, `urc connect` — Tailscale discovery + TLS forward + launch viewer. |
| `urc-coordinator` | Optional VPS. WebSocket relay between agents and clients. |
| `urc-common` | Shared config types, protocol messages, Tailscale helpers. |
| `urc-files` | HTTP file API (axum); agent can expose `files_root`. |

### Data path (happy path — Tailscale only)

```
Mac: Screen Sharing
  → localhost:15900 (plain VNC)
    → urc-client TLS forwarder
      → sa-grs:15900 (TLS)
        → urc-agent TlsTunnel
          → localhost:5900 (TigerVNC x0tigervncserver)
            → X11 display :N (user desktop)
```

### Agent internals (`urc-agent`)

1. **`SessionDetector`** (`session.rs`) — Uses `loginctl`, `who`, `/tmp/.X11-unix`, env fallbacks to find active X11/Wayland session and user.
2. **`BackendManager`** (`backend/`) — Pluggable VNC:
   - **X11** (`x11.rs`) — `x0tigervncserver` screen scrape (Ubuntu desktop default).
   - **GNOME Wayland** (`gnome.rs`) — `grdctl` / gnome-remote-desktop.
   - **wlroots** (`wayvnc.rs`) — `wayvnc`.
3. **`TlsTunnel`** (`tunnel.rs`) — Listens `0.0.0.0:15900`, terminates TLS, pipes to `127.0.0.1:5900`.
4. **`supervisor`** (`supervisor.rs`) — Loop: detect session → start backend → start TLS → health checks → backoff on failure.
5. **`CoordinatorClient`** (`coordinator.rs`) — Optional; disabled when `coordinator_url` is empty (Tailscale-only mode).

### Client internals (`urc-client`)

1. Resolve host via `urc_common::tailscale::list_peers` / `resolve_peer`.
2. **`tls_forward::preflight_remote_vnc`** — TLS to remote :15900, read RFB banner (`RFB …`).
3. **`spawn_tls_forward`** — Bind `127.0.0.1:15900`, for each local connection open TLS to remote and pipe bytes.
4. **`probe_local_vnc`** — Verify local port speaks RFB before opening viewer.
5. **macOS:** `open -a "Screen Sharing" vnc://localhost:15900` (with optional password in URL).

### Ports

| Port | Where | Purpose |
|------|--------|---------|
| 5900 | Agent localhost | TigerVNC (screen scrape) |
| 15900 | Agent `0.0.0.0` | TLS-wrapped VNC (tailnet clients) |
| 15901 | Agent localhost | Files HTTP (if enabled) |
| 21150 | Coordinator | WebSocket control + relay |

### Install / packaging

| Path | Purpose |
|------|---------|
| `install` (repo root) | curl entrypoint. Fetches scripts from GitHub commit SHA, source tarball, optional prebuilts. |
| `packaging/install.sh` | Linux agent/coordinator/client deps, systemd, VNC bootstrap. |
| `packaging/install-mac-client.sh` | macOS client only — copies binaries, Tailscale, `/usr/local/etc/urc/client.env`. |
| `packaging/scripts/urc` | CLI wrapper (finds `tailscale`, dispatches to `urc-client` / `urc-agent`). |
| `packaging/systemd/urc-agent.service` | `ExecStartPre=urc-fix-agent-perms.sh`, `wait-for-session.sh`, then `urc-agent`. |

**Important:** `URC_ALWAYS_SOURCE=1` is default in `install` — curl install builds from source so scripts and binaries match. Release **v0.1.12** prebuilts are **stale** relative to `main`.

---

## 3. Target machine context (sa-grs)

- Ubuntu desktop, user `german`, graphical session on **X11 display `:1`** (not `:0`).
- `loginctl` often leaves `Display=` empty for the session; fallback must use `who` or `/tmp/.X11-unix/X1`.
- GDM / long install retries once saturated X11 (**“Maximum number of clients reached”**, ~256 clients) — required **GNOME logout/login** once; agent retry loop made this worse before backoff was added.
- Tailscale hostname: **sa-grs**, IP **100.80.198.81**.

---

## 4. Issues encountered and fixes (on `main`, not necessarily in a release)

### Install / curl

| Issue | Cause | Fix (commit area) |
|-------|--------|-------------------|
| `Permission denied` on `/tmp/urc-install.*` | Root-owned workdir, cargo as user | `chown` workdir (Linux); Mac: user-owned `mktemp` |
| `extra[@]: unbound variable` on Mac bash 3.2 | Empty array under `set -u` | Rewrote `packaging/scripts/urc` |
| Prebuilt agent stale vs fresh scripts | curl used v0.1.12 binaries + main scripts | `URC_ALWAYS_SOURCE=1`, build from source |
| `runuser -H` invalid on Linux | BSD-only flag | Use `sudo -u` in `fix-agent.sh` |
| **`chown: user: illegal group name` on Mac** | (1) `chown user:user` when group is `staff`; (2) **`tar` restoring Linux ownership** from GitHub archive; (3) shell `chown` after extract | Mac: user-owned workdir, extract as user, no `chown` on Darwin (`a0f26d8` — **verify on user’s Mac**) |
| `Text file busy` on `cp urc-agent` | Copied while agent running | Stop agent before copy |

### Agent / VNC

| Issue | Cause | Fix |
|-------|--------|-----|
| Connection refused :15900 | Agent not listening; VNC never started | Bootstrap, supervisor, TigerVNC package |
| `x0vncserver not found` | Wrong package on Ubuntu 24.04 | `tigervnc-scraping-server` → `x0tigervncserver` |
| VNC as root / wrong user | Screen scrape needs desktop user | `runuser -u USER` + `DISPLAY`/`XAUTHORITY` |
| Wrong display `:0` | Fallback default when loginctl empty | `display_from_who()`, `display_from_x11_unix()` |
| `/etc/urc/vncpasswd` root-owned | Installer only chowned on create | `urc-fix-agent-perms.sh`, agent `ensure_password_file()` |
| X client saturation | Retry loop every 5s | Exponential backoff; `urc-recover-x11.sh` |
| Agent panic on start | axum 0.8 routes `/*path` invalid | `/{*path}` in `urc-files` |

### Mac client / connect

| Issue | Cause | Fix / status |
|-------|--------|--------------|
| `Connection failed to 127.0.0.1:15900` | Screen Sharing + VncAuth mismatch; or tunnel not actually VNC | Agent: `SecurityTypes None` on localhost VNC (TLS is the wire security); client: `vnc://localhost:…` |
| Tailscale not on PATH (Mac App Store) | Symlink to app binary crashes | Wrapper script in `/usr/local/bin/tailscale` |
| rustls panic | ring + aws-lc-rs | Unified on `ring` + `install_default()` |

### CI / releases

| Issue | Fix |
|-------|-----|
| Release uploaded manifest not tarballs | `release.yml` uploads real assets |
| v0.1.10 failed | ARM flake, parallel release race — split jobs, `ci.yml` |
| **Latest release v0.1.12** | Does **not** include most fixes above — **`main` is ahead** |

---

## 5. Current state (as of last session)

### Likely working on sa-grs (if user ran local build + restart)

- `urc-agent` active, `display=:1`, VNC on 5900, TLS on 15900 (verified in logs).
- User deployed via `sudo cp target/release/urc-agent` from checkout, not necessarily latest curl.

### Likely broken / unverified

- **Mac curl install** — user still reported `chown: user: illegal group name` after multiple fixes; last fix (`a0f26d8`) must be confirmed with installer banner `mac-tar-fix`.
- **End-to-end `urc connect`** — tunnel came up but Screen Sharing failed earlier; may work after agent `SecurityTypes None` + Mac client rebuild.
- **README** still points at `releases/latest/download/install` — should use **`main/install`** until **v0.1.13+** tag.

---

## 6. Recommended next steps (priority order)

1. **Verify Mac install** — User must see `installer mac-tar-fix`. If not, caching or wrong URL. Consider hosting install script only from tagged releases after CI green.
2. **Tag v0.1.13** — Build linux + darwin prebuilts from current `main`; attach `install` + `install-mac-client.sh` to release so `releases/latest` works.
3. **Confirm e2e** — From Mac: `urc connect sa-grs` with agent on `main` binary; capture `journalctl -u urc-agent` and Screen Sharing behavior.
4. **Unify install URL** — README + docs: one canonical URL; document `URC_ALWAYS_SOURCE=0` for fast prebuilt path once releases are current.
5. **Reduce install fragility on Mac** — Consider shipping **darwin prebuilt only** for client (no cargo on Mac) once release pipeline is trusted; keep source build as fallback.
6. **Agent: don’t hammer X on failure** — Backoff is in place; consider max retry cap and clear log message pointing to `urc-recover-x11.sh`.
7. **Tests** — No automated e2e; add smoke test: TLS port returns RFB banner; session detection unit tests for `display_from_who`.

---

## 7. Key files to read first

```
install                          # curl entry (Mac vs Linux branching)
packaging/install.sh             # agent install, VNC deps, bootstrap 15900
packaging/install-mac-client.sh  # Mac client
crates/urc-agent/src/session.rs  # display :0 vs :1
crates/urc-agent/src/backend/x11.rs
crates/urc-agent/src/tunnel.rs
crates/urc-client/src/tls_forward.rs
crates/urc-client/src/main.rs    # connect + Screen Sharing
packaging/scripts/urc-recover-x11.sh
fix-agent.sh                     # local dev: build + deploy + recover
```

---

## 8. Commands cheat sheet

```bash
# Agent (PC) — from main
curl -fsSL https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install | \
  sudo bash -s -- --role agent -y

# Client (Mac) — from main (must show installer mac-tar-fix)
curl -fsSL "https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install?t=$(date +%s)" | \
  sudo bash -s -- --role client -y

# Connect (Mac)
urc hosts
urc connect sa-grs

# Debug (PC)
sudo systemctl status urc-agent
ss -tlnp | grep -E '5900|15900'
journalctl -u urc-agent -n 50 --no-pager
cat ~/.vnc/$(hostname -s):1.log

# Recover X saturation (PC, no GDM restart)
sudo /path/to/packaging/scripts/urc-recover-x11.sh
```

---

## 9. Process note

This project accumulated many **interactive fixes** on a live machine (`sa-grs`) while **release artifacts lagged `main`**. The dominant failure mode for “1-click” was **script/binary mismatch** plus **macOS-specific install edge cases** (tar ownership, chown, bash 3.2, Screen Sharing vs VNC auth). Future work should treat **a green release tag** as the definition of “shippable,” not just green `main`.
