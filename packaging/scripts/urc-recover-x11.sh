#!/usr/bin/env bash
# Recover X11 + VNC for urc-agent without restarting GDM.
set -euo pipefail

DESKTOP_USER="${SUDO_USER:-${USER}}"
if [[ "$(id -u)" -ne 0 ]]; then
  exec sudo -E bash "$0" "$@"
fi

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

install -d -m755 /usr/libexec/urc
if [[ -f "$REPO_ROOT/packaging/scripts/urc-fix-agent-perms.sh" ]]; then
  install -m755 "$REPO_ROOT/packaging/scripts/urc-fix-agent-perms.sh" /usr/libexec/urc/urc-fix-agent-perms.sh
fi
if [[ -f "$REPO_ROOT/packaging/systemd/urc-agent.service" ]]; then
  install -m644 "$REPO_ROOT/packaging/systemd/urc-agent.service" /etc/systemd/system/urc-agent.service
  systemctl daemon-reload
fi

if [[ -f "$REPO_ROOT/target/release/urc-agent" ]]; then
  echo "==> Installing urc-agent binary"
  systemctl stop urc-agent.service 2>/dev/null || true
  install -m755 "$REPO_ROOT/target/release/urc-agent" /usr/local/bin/urc-agent
fi

echo "==> Stopping urc-agent (stops VNC retry loop)"
systemctl stop urc-agent.service 2>/dev/null || true

echo "==> Clearing stale TigerVNC processes for ${DESKTOP_USER}"
pkill -u "$DESKTOP_USER" -f 'x0tigervnc|x0vncserver' 2>/dev/null || true
runuser -u "$DESKTOP_USER" -- bash -lc 'rm -f "$HOME/.vnc"/*.pid 2>/dev/null; true'

if [[ -x /usr/libexec/urc/urc-fix-agent-perms.sh ]]; then
  /usr/libexec/urc/urc-fix-agent-perms.sh
fi

DISPLAY="$(who | awk -v u="$DESKTOP_USER" '$1==u && $2 ~ /^:/ {print $2; exit}')"
[[ -z "$DISPLAY" ]] && DISPLAY=":1"
XAUTH="/run/user/$(id -u "$DESKTOP_USER")/gdm/Xauthority"

echo "==> Waiting for X display ${DISPLAY} (up to 90s)…"
for i in $(seq 1 18); do
  if runuser -u "$DESKTOP_USER" -- env DISPLAY="$DISPLAY" XAUTHORITY="$XAUTH" xdpyinfo >/dev/null 2>&1; then
    echo "==> X display ${DISPLAY} is reachable"
    break
  fi
  if [[ "$i" -eq 18 ]]; then
    echo "ERROR: ${DISPLAY} not reachable. Log out and back into GNOME, then re-run: sudo $0" >&2
    exit 1
  fi
  sleep 5
done

echo "==> Starting urc-agent"
systemctl start urc-agent.service

for i in $(seq 1 30); do
  if ss -tlnp 2>/dev/null | grep -qE ':15900[[:space:]]'; then
    echo "==> Remote desktop ready (port 15900)"
    ss -tlnp 2>/dev/null | grep 15900 || true
    exit 0
  fi
  sleep 2
done

echo "ERROR: port 15900 did not open. Logs:" >&2
journalctl -u urc-agent -n 30 --no-pager >&2 || true
exit 1
