#!/usr/bin/env bash
# Periodic health watchdog for urc-agent (systemd timer or cron).
set -euo pipefail

AGENT_BIN="${URC_AGENT_BIN:-/usr/local/bin/urc-agent}"
CONFIG="${URC_CONFIG:-/etc/urc/agent.toml}"
UNIT="urc-agent.service"
TAG="urc-health"
# Serving ports. WEB_TLS_PORT (15901) is the EXTERNAL web TLS port remote clients
# hit (tunnel -> 16080); if it is down remote access is a silent outage even when
# 16080 is fine, so a down 15901 alone is a failure. WEB_INTERNAL_PORT (16080) is
# probed for diagnostics/logging only.
WEB_TLS_PORT="${URC_WEB_TLS_PORT:-15901}"
WEB_INTERNAL_PORT="${URC_WEB_INTERNAL_PORT:-16080}"
STATUS_FILE="${URC_STATUS_FILE:-/run/urc/status.json}"

log() { logger -t "$TAG" "$*"; }

restart_agent() {
  log "unhealthy — restarting $UNIT"
  systemctl restart "$UNIT" || log "failed to restart $UNIT"
}

# Returns 0 if something is listening on the given TCP port.
port_listening() {
  local port="$1"
  if command -v ss >/dev/null 2>&1; then
    ss -ltn "( sport = :$port )" 2>/dev/null | grep -q ":$port"
  else
    # Fallback: bash /dev/tcp probe.
    (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && exec 3>&- 3<&-
  fi
}

# Returns 0 if the agent reports a graphical session is present. Before the first
# login the agent legitimately waits (no session) and binds NO serving ports, so
# absent ports are expected and must NOT trigger a restart. We only treat
# "ports down" as a failure when a session IS detected (agent should be serving
# but isn't). Missing/unreadable/"waiting" status => treat as no session.
# `grep` under `set -e` is guarded with `|| true` so a no-match never aborts.
session_detected() {
  [ -r "$STATUS_FILE" ] || return 1
  local val
  val="$(grep -o '"session_detected"[[:space:]]*:[[:space:]]*true' "$STATUS_FILE" 2>/dev/null || true)"
  [ -n "$val" ]
}

if ! systemctl is-active --quiet "$UNIT"; then
  log "$UNIT not active — starting"
  systemctl start "$UNIT" || restart_agent
  exit 0
fi

# Direct serving probe — gated on session presence (M2). Before the first
# graphical session the agent binds no serving ports and is correctly waiting for
# login, so absent ports are NOT a failure. Only when a session is detected does
# "external web TLS port (15901) down" mean the agent should be serving but isn't
# (M3b): 15901 is what remote clients hit, so it alone being down is a silent
# outage even if 16080 is up. 16080 is probed for diagnostics only.
web_tls_up=true
port_listening "$WEB_TLS_PORT" || web_tls_up=false
web_internal_up=true
port_listening "$WEB_INTERNAL_PORT" || web_internal_up=false

if session_detected; then
  if [ "$web_tls_up" = false ]; then
    log "session present but external web TLS port down (15901=down 16080=$([ "$web_internal_up" = true ] && echo up || echo down)) — restarting"
    restart_agent
    exit 1
  fi
else
  # No session yet: agent is waiting for login. Don't restart on absent ports;
  # defer to `health`, which distinguishes waiting-for-session from unhealthy.
  log "no session detected — skipping serving-port restart (15901=$([ "$web_tls_up" = true ] && echo up || echo down) 16080=$([ "$web_internal_up" = true ] && echo up || echo down))"
fi

if ! "$AGENT_BIN" health --config "$CONFIG"; then
  restart_agent
  exit 1
fi

log "$UNIT OK"
exit 0
