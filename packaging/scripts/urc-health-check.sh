#!/usr/bin/env bash
# Periodic health watchdog for urc-agent (systemd timer or cron).
set -euo pipefail

AGENT_BIN="${URC_AGENT_BIN:-/usr/local/bin/urc-agent}"
CONFIG="${URC_CONFIG:-/etc/urc/agent.toml}"
UNIT="urc-agent.service"
TAG="urc-health"

log() { logger -t "$TAG" "$*"; }

restart_agent() {
  log "unhealthy — restarting $UNIT"
  systemctl restart "$UNIT" || log "failed to restart $UNIT"
}

if ! systemctl is-active --quiet "$UNIT"; then
  log "$UNIT not active — starting"
  systemctl start "$UNIT" || restart_agent
  exit 0
fi

if ! "$AGENT_BIN" health --config "$CONFIG"; then
  restart_agent
  exit 1
fi

log "$UNIT OK"
exit 0
