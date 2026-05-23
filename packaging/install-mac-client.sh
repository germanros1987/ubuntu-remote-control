#!/usr/bin/env bash
# macOS URC client installer — no BSD install(1), no apt.
set -euo pipefail

INSTALL_PREFIX="/usr/local"
URC_ETC="/usr/local/etc/urc"
CLIENT_ENV="$URC_ETC/client.env"
URC_BIN_DIR="${URC_BIN_DIR:-}"
NONINTERACTIVE=false
COORDINATOR_URL=""
TAILSCALE_AUTH_KEY="${URC_TAILSCALE_AUTH_KEY:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --coordinator-url) COORDINATOR_URL="$2"; shift 2 ;;
    --tailscale-auth-key) TAILSCALE_AUTH_KEY="$2"; shift 2 ;;
    -y|--yes) NONINTERACTIVE=true; shift ;;
    *) shift ;;
  esac
done

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Run as root: sudo bash ..." >&2
  exit 1
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script is for macOS only." >&2
  exit 1
fi

if [[ -z "$URC_BIN_DIR" ]] || [[ ! -f "$URC_BIN_DIR/urc-client" ]]; then
  echo "ERROR: missing prebuilt binaries in URC_BIN_DIR" >&2
  exit 1
fi

prompt() {
  local msg="$1" default="${2:-}"
  if $NONINTERACTIVE; then
    echo "$default"
    return
  fi
  [[ -r /dev/tty ]] || { echo "$default"; return; }
  local ans=""
  printf '%s [%s]: ' "$msg" "$default" >/dev/tty
  read -r ans </dev/tty || true
  echo "${ans:-$default}"
}

echo ""
echo "=== Ubuntu Remote Control (macOS client) ==="
echo ""

if ! $NONINTERACTIVE; then
  relay="$(prompt "Use a VPS relay instead of Tailscale-only? [y/N]" "N")"
  if [[ "$relay" =~ ^[Yy] ]]; then
    COORDINATOR_URL="$(prompt "Coordinator WebSocket URL" "ws://YOUR_VPS:21150/ws/client")"
  fi
fi

echo "==> Installing URC client to $INSTALL_PREFIX/bin"
mkdir -p "$INSTALL_PREFIX/bin" "$URC_ETC"
cp -f "$URC_BIN_DIR/urc-client" "$INSTALL_PREFIX/bin/urc-client"
cp -f "$URC_BIN_DIR/urc" "$INSTALL_PREFIX/bin/urc"
chmod 755 "$INSTALL_PREFIX/bin/urc-client" "$INSTALL_PREFIX/bin/urc"

echo "==> macOS client — uses built-in Screen Sharing (no extra VNC app)"

echo "==> Installing Tailscale"
if ! command -v tailscale >/dev/null 2>&1; then
  curl -fsSL https://tailscale.com/install.sh | sh
fi

ts_user="${SUDO_USER:-$USER}"
hn="$(hostname -s)"

if ! tailscale status --json 2>/dev/null | grep -qE '"BackendState":"Running"|"State":10'; then
  if [[ -n "$TAILSCALE_AUTH_KEY" ]]; then
    sudo -u "$ts_user" tailscale up --auth-key="$TAILSCALE_AUTH_KEY" --accept-routes
  elif ! $NONINTERACTIVE && [[ -r /dev/tty ]]; then
    echo "Sign in to Tailscale (open the URL below if prompted):" >/dev/tty
    sudo -u "$ts_user" tailscale up </dev/tty >/dev/tty 2>&1 || true
  else
    echo "Run: tailscale up"
  fi
else
  echo "Tailscale already connected."
fi
sudo -u "$ts_user" tailscale set --hostname="$hn" 2>/dev/null || true

TOKEN="$(openssl rand -hex 24)"
cat > "$CLIENT_ENV" <<EOF
URC_COORDINATOR=$COORDINATOR_URL
URC_TOKEN=$TOKEN
URC_DEFAULT_HOST=
EOF
chmod 644 "$CLIENT_ENV"

echo ""
echo "============================================"
echo "  Client ready"
echo "============================================"
echo ""
echo "  Config: $CLIENT_ENV"
echo "  Viewer: built-in Screen Sharing (opens automatically)"
echo ""
echo "  List your PCs:  urc hosts"
echo "  Connect:        urc connect NAME"
echo ""
