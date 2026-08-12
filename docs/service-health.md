# URC service health and auto-recovery

Remote access must survive reboots, crashed VNC backends, and dropped coordinator connections. URC uses **three layers** of protection.

## Layer 1 — systemd

Configured in [`packaging/systemd/urc-agent.service`](../packaging/systemd/urc-agent.service):

| Setting | Value |
|---------|-------|
| `Restart` | `always` |
| `RestartSec` | `10` |
| `StartLimitIntervalSec` | `0` (no permanent lockout) |
| `TimeoutStartSec` | `300` |
| `ExecStartPre` | `wait-for-session.sh` |
| `KillMode` | `control-group` (kills VNC children on restart) |

On install, these units are enabled:

- `urc-agent.service` — main agent
- `urc-agent-health.timer` — watchdog every **2 minutes** (first run 2 min after boot)
- `urc-agent-login.path` — `try-restart` agent when login sessions change

Coordinator VPS uses the same pattern (`urc-coordinator.service` + `urc-coordinator-health.timer`).

## Layer 2 — in-process supervisor

`urc-agent` runs a supervisor loop ([`crates/urc-agent/src/supervisor.rs`](../crates/urc-agent/src/supervisor.rs)):

1. Poll until a graphical session exists (every 30s).
2. Start VNC backend, files API, TLS tunnel, coordinator WebSocket.
3. Every 30s: check VNC port + coordinator connection.
4. On failure: tear down, pause 5s, repeat.

Status is written to **`/run/urc/status.json`** for external checks.

## Layer 3 — periodic watchdog

[`packaging/scripts/urc-health-check.sh`](../packaging/scripts/urc-health-check.sh) runs via systemd timer:

```bash
systemctl is-active urc-agent   # start if down
urc-agent health                # restart if unhealthy
```

Equivalent to cron `*/2 * * * *` but integrated with journald (`journalctl -t urc-health`).

## Commands

```bash
# Service state
systemctl status urc-agent
journalctl -u urc-agent -f

# JSON health (supervisor view)
urc-agent status

# Exit code for scripts (0 = healthy)
urc-agent health

# Timer
systemctl list-timers urc-agent-health.timer
```

## Testing recovery

| Test | Expected |
|------|----------|
| `sudo reboot` | Agent healthy within ~5 min (session wait + timer) |
| `sudo killall x0vncserver` | Supervisor restarts stack within ~30s |
| `sudo kill -9 $(pidof urc-agent)` | systemd restarts agent within ~10s |
| Stop VPS coordinator | Agent reconnects when coordinator returns; timer fixes stuck state within 10 min |

## Coordinator (VPS)

```bash
systemctl status urc-coordinator
systemctl status urc-coordinator-health.timer
journalctl -t urc-coordinator-health
```

Ensure `/etc/urc/coordinator.env` contains `URC_SHARED_SECRET=your-token`.

## Optional cron fallback

If you prefer cron over systemd timers:

```cron
*/2 * * * * root /usr/libexec/urc/urc-health-check.sh
```

Disable the timer to avoid duplicate checks: `systemctl disable --now urc-agent-health.timer`.
