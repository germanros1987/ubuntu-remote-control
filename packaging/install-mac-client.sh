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

ts_user="${SUDO_USER:-$USER}"

# Apple Screen Sharing rejects TigerVNC's RFB handshake ("software ... incompatible").
# Install TigerVNC Viewer for matching client; Screen Sharing remains as fallback.
#
# The Homebrew cask renamed the app from "TigerVNC Viewer.app" (<=1.15) to
# "TigerVNC.app" (>=1.16), so detection has to glob.
find_tigervnc_app() {
  local app
  for app in /Applications/TigerVNC*.app; do
    [[ -d "$app" ]] || continue
    if [[ -x "$app/Contents/MacOS/vncviewer" ]]; then
      echo "$app/Contents/MacOS/vncviewer"
      return 0
    fi
  done
  return 1
}

install_tigervnc_viewer() {
  if find_tigervnc_app >/dev/null; then
    echo "==> TigerVNC Viewer already installed"
    return 0
  fi
  echo "==> Installing TigerVNC Viewer (matches agent's VNC server for reliable connect)"
  if ! sudo -u "$ts_user" -H bash -lc 'command -v brew >/dev/null 2>&1'; then
    echo "==> Installing Homebrew"
    if ! sudo -u "$ts_user" -H bash -lc \
        'NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"'; then
      echo "WARN: Homebrew install failed; client will fall back to Screen Sharing" >&2
      return 1
    fi
  fi
  if ! sudo -u "$ts_user" -H bash -lc 'brew install --cask tigervnc'; then
    echo "WARN: TigerVNC Viewer install failed; client will fall back to Screen Sharing" >&2
    return 1
  fi
  return 0
}
install_tigervnc_viewer || true

# Symlink the actual TigerVNC binary into PATH so `which vncviewer` finds it and
# urc-client routes through the generic VNC launcher instead of Apple Screen Sharing.
VNC_BIN="$(find_tigervnc_app || true)"
if [[ -n "$VNC_BIN" ]]; then
  ln -sf "$VNC_BIN" "$INSTALL_PREFIX/bin/vncviewer"
  echo "==> Linked TigerVNC Viewer -> $INSTALL_PREFIX/bin/vncviewer"
  # Strip Gatekeeper quarantine so first launch does not block.
  app_dir="${VNC_BIN%/Contents/MacOS/vncviewer}"
  xattr -dr com.apple.quarantine "$app_dir" 2>/dev/null || true
fi

echo "==> Installing Tailscale"
if ! command -v tailscale >/dev/null 2>&1; then
  curl -fsSL https://tailscale.com/install.sh | sh
fi

# App Store CLI crashes when invoked via symlink — install a small wrapper instead.
MACOS_TS_APP="/Applications/Tailscale.app/Contents/MacOS/Tailscale"
MACOS_TS_CLI="$INSTALL_PREFIX/bin/tailscale"

tailscale_cli_works() {
  command -v tailscale >/dev/null 2>&1 \
    && tailscale version >/dev/null 2>&1
}

install_macos_tailscale_wrapper() {
  [[ -x "$MACOS_TS_APP" ]] || return 1
  cat > "$MACOS_TS_CLI" <<EOF
#!/bin/sh
exec "$MACOS_TS_APP" "\$@"
EOF
  chmod 755 "$MACOS_TS_CLI"
  echo "==> Installed Tailscale CLI wrapper → $MACOS_TS_CLI"
}

ensure_tailscale_cli() {
  if tailscale_cli_works; then
    return 0
  fi
  # Remove broken symlink from older URC installs.
  if [[ -L "$MACOS_TS_CLI" ]] && [[ "$(readlink "$MACOS_TS_CLI")" == *Tailscale.app* ]]; then
    rm -f "$MACOS_TS_CLI"
  fi
  install_macos_tailscale_wrapper || {
    echo "WARN: Install Tailscale from https://tailscale.com/download/mac and sign in via the menu bar." >&2
    return 1
  }
}
ensure_tailscale_cli || true

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

TS_BIN=""
if [[ -x "$MACOS_TS_APP" ]]; then
  TS_BIN="$MACOS_TS_APP"
elif command -v tailscale >/dev/null 2>&1; then
  TS_BIN="$(command -v tailscale)"
fi
TOKEN="$(openssl rand -hex 24)"
{
  echo "URC_COORDINATOR=$COORDINATOR_URL"
  echo "URC_TOKEN=$TOKEN"
  echo "URC_DEFAULT_HOST="
  [[ -n "$TS_BIN" ]] && echo "URC_TAILSCALE_BIN=$TS_BIN"
} > "$CLIENT_ENV"
chmod 644 "$CLIENT_ENV"

echo ""
echo "============================================"
echo "  Client ready"
echo "============================================"
echo ""
echo "  Config: $CLIENT_ENV"
if [[ -x "$INSTALL_PREFIX/bin/vncviewer" ]] || find_tigervnc_app >/dev/null; then
  echo "  Viewer: TigerVNC Viewer (opens automatically)"
else
  echo "  Viewer: built-in Screen Sharing (TigerVNC Viewer install failed — may hit compatibility errors)"
fi
echo ""
echo "  List your PCs:  urc hosts"
echo "  Connect:        urc connect NAME"
echo ""
