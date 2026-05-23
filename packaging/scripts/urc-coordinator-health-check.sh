#!/usr/bin/env bash
# Periodic health watchdog for urc-coordinator.
set -euo pipefail

UNIT="urc-coordinator.service"
PORT="${URC_COORDINATOR_PORT:-21150}"
TAG="urc-coordinator-health"

log() { logger -t "$TAG" "$*"; }

restart_coord() {
  log "unhealthy — restarting $UNIT"
  systemctl restart "$UNIT" || log "failed to restart $UNIT"
}

if ! systemctl is-active --quiet "$UNIT"; then
  log "$UNIT not active — starting"
  systemctl start "$UNIT" || restart_coord
  exit 0
fi

if ! ss -ltn 2>/dev/null | grep -q ":${PORT} "; then
  log "port $PORT not listening"
  restart_coord
  exit 1
fi

# Hosts API sanity check
if command -v curl >/dev/null 2>&1; then
  if ! curl -sf --max-time 5 "http://127.0.0.1:${PORT}/hosts" >/dev/null; then
    log "/hosts probe failed"
    restart_coord
    exit 1
  fi
fi

log "$UNIT OK"
exit 0
