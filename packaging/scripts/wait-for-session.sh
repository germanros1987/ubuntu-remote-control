#!/usr/bin/env bash
# Wait for an active graphical session before starting urc-agent.
# Used as ExecStartPre. Exits 0 even on timeout so the agent supervisor can keep polling.
set -euo pipefail

TIMEOUT="${URC_SESSION_WAIT_TIMEOUT:-300}"
INTERVAL=5
elapsed=0

has_graphical_session() {
  if ! command -v loginctl >/dev/null 2>&1; then
    return 1
  fi
  while read -r sid _ uid _; do
    [[ -z "${sid:-}" ]] && continue
    [[ "${uid:-0}" == "0" ]] && continue
    local stype
    stype=$(loginctl show-session "$sid" -p Type --value 2>/dev/null || true)
    local active
    active=$(loginctl show-session "$sid" -p Active --value 2>/dev/null || true)
    if [[ "$active" == "yes" && ( "$stype" == "x11" || "$stype" == "wayland" ) ]]; then
      return 0
    fi
  done < <(loginctl list-sessions --no-legend 2>/dev/null || true)
  return 1
}

logger -t urc-wait-session "waiting up to ${TIMEOUT}s for graphical session"

while (( elapsed < TIMEOUT )); do
  if has_graphical_session; then
    logger -t urc-wait-session "graphical session detected after ${elapsed}s"
    exit 0
  fi
  sleep "$INTERVAL"
  elapsed=$((elapsed + INTERVAL))
done

logger -t urc-wait-session "timeout after ${TIMEOUT}s — starting agent anyway (supervisor will retry)"
exit 0
