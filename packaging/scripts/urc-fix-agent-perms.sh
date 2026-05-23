#!/usr/bin/env bash
# Fix permissions before urc-agent starts (runs as root from systemd).
set -euo pipefail

if [[ ! -f /etc/urc/vncpasswd ]]; then
  exit 0
fi

desktop_user=""
if command -v loginctl >/dev/null 2>&1; then
  while read -r _sid uid _rest; do
    [[ "$uid" =~ ^[0-9]+$ ]] || continue
    [[ "$uid" -eq 0 ]] && continue
    state=$(loginctl show-session "$_sid" -p State --value 2>/dev/null || true)
    active=$(loginctl show-session "$_sid" -p Active --value 2>/dev/null || true)
    [[ "$state" == "active" && "$active" == "yes" ]] || continue
    desktop_user=$(loginctl show-session "$_sid" -p Name --value 2>/dev/null || true)
    [[ -n "$desktop_user" ]] && break
  done < <(loginctl list-sessions --no-legend 2>/dev/null || true)
fi

if [[ -z "$desktop_user" ]]; then
  desktop_user=$(who 2>/dev/null | awk '$2 ~ /^:/ {print $1; exit}')
fi

if [[ -z "$desktop_user" ]] || ! id "$desktop_user" &>/dev/null; then
  exit 0
fi

chown "$desktop_user:$desktop_user" /etc/urc/vncpasswd
chmod 600 /etc/urc/vncpasswd
