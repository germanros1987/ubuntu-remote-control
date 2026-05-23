#!/usr/bin/env bash
# Ubuntu Remote Control — one-command installer
#
#   sudo ./install                    # interactive (recommended)
#   sudo ./install --role agent       # this PC — remote into it
#   sudo ./install --role coordinator # VPS relay
#   sudo ./install --role client      # laptop you connect from
#
set -euo pipefail

ROLE=""
PROFILE=""
GPU="auto"
TOKEN=""
COORDINATOR_URL=""
HOST_ID=""
WITH_TAILSCALE=false
NONINTERACTIVE=false
INSTALL_PREFIX="/usr/local"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CREDENTIALS_FILE="/etc/urc/credentials"

usage() {
  cat <<'EOF'
Ubuntu Remote Control — installer

  sudo ./install                     Interactive wizard (1-click style)
  sudo ./install --role coordinator  Relay server (VPS, port 21150)
  sudo ./install --role agent        This machine — allow remote desktop
  sudo ./install --role client       This machine — connect to a remote PC

Options:
  --coordinator-url URL   ws://your-vps:21150/ws/agent (agent) or .../ws/client
  --host-id NAME          Name to use when connecting (default: hostname)
  --token SECRET          Shared secret (auto-generated if omitted)
  --gpu auto|nvidia|intel|amd   For headless agent only
  --with-tailscale
  -y, --yes               Non-interactive; accept defaults
  -h, --help

After install, connect with:
  urc connect HOST_ID
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --role) ROLE="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --gpu) GPU="$2"; shift 2 ;;
    --token) TOKEN="$2"; shift 2 ;;
    --coordinator-url) COORDINATOR_URL="$2"; shift 2 ;;
    --host-id) HOST_ID="$2"; shift 2 ;;
    --with-tailscale) WITH_TAILSCALE=true; shift ;;
    --prefix) INSTALL_PREFIX="$2"; shift 2 ;;
    -y|--yes) NONINTERACTIVE=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
done

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Run as root: sudo ./install" >&2
  exit 1
fi

prompt() {
  local msg="$1" default="${2:-}"
  if $NONINTERACTIVE; then
    echo "${default}"
    return
  fi
  if [[ -n "$default" ]]; then
    read -r -p "$msg [$default]: " ans || true
    echo "${ans:-$default}"
  else
    read -r -p "$msg: " ans || true
    echo "$ans"
  fi
}

has_desktop() {
  dpkg -l ubuntu-desktop-minimal 2>/dev/null | grep -q '^ii' || \
  dpkg -l ubuntu-desktop 2>/dev/null | grep -q '^ii' || \
  dpkg -l gnome-shell 2>/dev/null | grep -q '^ii'
}

ensure_token() {
  install -d -m755 /etc/urc
  if [[ -n "$TOKEN" ]]; then
    return
  fi
  if [[ -f /etc/urc/token ]]; then
    TOKEN="$(cat /etc/urc/token)"
    return
  fi
  TOKEN="$(openssl rand -hex 24)"
  echo "$TOKEN" > /etc/urc/token
  chmod 600 /etc/urc/token
}

save_credentials() {
  local coord_pub="${1:-}"
  install -d -m755 /etc/urc
  cat > "$CREDENTIALS_FILE" <<EOF
# Ubuntu Remote Control — save these somewhere safe
URC_TOKEN=$TOKEN
URC_COORDINATOR_PUBLIC=$coord_pub
URC_HOST_ID=${HOST_ID:-$(hostname -s)}
EOF
  chmod 600 "$CREDENTIALS_FILE"
}

write_client_config() {
  local coord_client="$1"
  local target_host="${2:-}"
  install -d -m755 /etc/urc
  cat > /etc/urc/client.env <<EOF
URC_COORDINATOR=$coord_client
URC_TOKEN=$TOKEN
URC_DEFAULT_HOST=$target_host
EOF
  chmod 644 /etc/urc/client.env
}

run_interactive() {
  echo ""
  echo "=== Ubuntu Remote Control ==="
  echo ""
  echo "What is this machine?"
  echo "  1) PC I remote INTO (home/office Ubuntu)"
  echo "  2) Relay server (small VPS with public IP)"
  echo "  3) Laptop I connect FROM"
  echo ""
  local choice
  choice="$(prompt "Choose 1-3" "1")"
  case "$choice" in
    1) ROLE=agent ;;
    2) ROLE=coordinator ;;
    3) ROLE=client ;;
    *) echo "Invalid choice"; exit 1 ;;
  esac
  echo ""
}

configure_role_interactive() {
  ensure_token
  case "$ROLE" in
    coordinator)
      echo "Coordinator will listen on port 21150 (open this in your VPS firewall)."
      ;;
    agent)
      if has_desktop; then
        PROFILE=minimal
        echo "Desktop detected — installing agent only (no extra desktop packages)."
      else
        local headless
        headless="$(prompt "Headless server (install full desktop stack)? [y/N]" "N")"
        if [[ "$headless" =~ ^[Yy] ]]; then
          PROFILE=desktop
          GPU="$(prompt "GPU type (auto/nvidia/intel/amd)" "auto")"
        else
          PROFILE=minimal
        fi
      fi
      HOST_ID="$(prompt "Name for this PC (used when connecting)" "$(hostname -s)")"
      if [[ -z "$COORDINATOR_URL" ]]; then
        local pub
        pub="$(prompt "Coordinator WebSocket URL" "ws://YOUR_VPS_IP:21150/ws/agent")"
        COORDINATOR_URL="$pub"
      fi
      ;;
    client)
      if [[ -z "$COORDINATOR_URL" ]]; then
        COORDINATOR_URL="$(prompt "Coordinator WebSocket URL" "ws://YOUR_VPS_IP:21150/ws/client")"
      fi
      HOST_ID="$(prompt "Host ID to connect to" "")"
      ;;
  esac
}

# --- deps & build ---

echo "==> Installing system packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq curl ca-certificates openssl

install_build_deps() {
  if ! command -v cargo >/dev/null 2>&1; then
    apt-get install -y -qq cargo rustc
  fi
}

build_and_install_binaries() {
  if [[ -n "${URC_SKIP_BUILD:-}" ]] && [[ -n "${URC_BIN_DIR:-}" ]]; then
    echo "==> Installing prebuilt binaries"
    install -Dm755 "$URC_BIN_DIR/urc-agent" "$INSTALL_PREFIX/bin/urc-agent"
    install -Dm755 "$URC_BIN_DIR/urc-client" "$INSTALL_PREFIX/bin/urc-client"
    install -Dm755 "$URC_BIN_DIR/urc-coordinator" "$INSTALL_PREFIX/bin/urc-coordinator"
    install -Dm755 "$URC_BIN_DIR/urc" "$INSTALL_PREFIX/bin/urc"
    return
  fi
  install_build_deps
  cd "$REPO_ROOT"
  echo "==> Building URC (first install — may take a few minutes)"
  cargo build --release
  install -Dm755 target/release/urc-agent "$INSTALL_PREFIX/bin/urc-agent"
  install -Dm755 target/release/urc-client "$INSTALL_PREFIX/bin/urc-client"
  install -Dm755 target/release/urc-coordinator "$INSTALL_PREFIX/bin/urc-coordinator"
  install -Dm755 "$REPO_ROOT/packaging/scripts/urc" "$INSTALL_PREFIX/bin/urc"
}

install_agent_deps() {
  apt-get install -y -qq tigervnc-standalone-server tigervnc-common
}

install_desktop_deps() {
  apt-get install -y -qq \
    xserver-xorg xserver-xorg-video-all \
    lightdm ubuntu-desktop-minimal \
    gnome-remote-desktop
  case "$GPU" in
    nvidia) apt-get install -y -qq nvidia-driver-535 || apt-get install -y -qq nvidia-driver-550 ;;
    intel) apt-get install -y -qq intel-media-va-driver mesa-va-drivers ;;
    amd) apt-get install -y -qq mesa-vulkan-drivers mesa-va-drivers ;;
    auto) lspci 2>/dev/null | grep -qi nvidia && apt-get install -y -qq nvidia-driver-535 || true ;;
  esac
  local u="${SUDO_USER:-ubuntu}"
  if [[ -f /etc/lightdm/lightdm.conf ]] && ! grep -q '^autologin-user=' /etc/lightdm/lightdm.conf; then
    sed -i "/^\[Seat:\*\]/a autologin-user=$u\nautologin-user-timeout=0" /etc/lightdm/lightdm.conf || true
  fi
  systemctl enable lightdm 2>/dev/null || true
}

install_client_deps() {
  apt-get install -y -qq tigervnc-viewer || true
}

setup_vnc_password() {
  if [[ -f /etc/urc/vncpasswd ]]; then
    return
  fi
  local vnc_pass
  vnc_pass="$(openssl rand -base64 12)"
  install -d -m755 /etc/urc
  if printf '%s\n' "$vnc_pass" | vncpasswd -f /etc/urc/vncpasswd 2>/dev/null; then
    chmod 600 /etc/urc/vncpasswd
    echo "$vnc_pass" >> "$CREDENTIALS_FILE"
    echo "URC_VNC_PASSWORD=$vnc_pass" >> "$CREDENTIALS_FILE"
  else
  echo "Note: create VNC password manually: sudo vncpasswd /etc/urc/vncpasswd"
  fi
}

setup_agent_config() {
  install -d -m755 /etc/urc
  "$INSTALL_PREFIX/bin/urc-agent" --init-config > /etc/urc/agent.toml
  local hid="${HOST_ID:-$(hostname -s)}"
  sed -i "s/^host_id = .*/host_id = \"$hid\"/" /etc/urc/agent.toml
  sed -i "s/^token = .*/token = \"$TOKEN\"/" /etc/urc/agent.toml
  sed -i "s|^coordinator_url = .*|coordinator_url = \"$COORDINATOR_URL\"|" /etc/urc/agent.toml
  install -d -m700 /etc/urc/tls
  setup_vnc_password
}

setup_coordinator_config() {
  install -d -m755 /etc/urc
  echo "URC_SHARED_SECRET=$TOKEN" > /etc/urc/coordinator.env
  chmod 600 /etc/urc/coordinator.env
}

install_libexec() {
  install -d -m755 /usr/libexec/urc
  install -Dm755 "$REPO_ROOT/packaging/scripts/wait-for-session.sh" /usr/libexec/urc/wait-for-session.sh
  install -Dm755 "$REPO_ROOT/packaging/scripts/urc-health-check.sh" /usr/libexec/urc/urc-health-check.sh
  install -Dm755 "$REPO_ROOT/packaging/scripts/urc-coordinator-health-check.sh" /usr/libexec/urc/urc-coordinator-health-check.sh
}

install_systemd_units() {
  local udir="$REPO_ROOT/packaging/systemd"
  install -Dm644 "$udir/urc-agent.service" /etc/systemd/system/urc-agent.service
  install -Dm644 "$udir/urc-coordinator.service" /etc/systemd/system/urc-coordinator.service
  install -Dm644 "$udir/urc-agent-health.service" /etc/systemd/system/urc-agent-health.service
  install -Dm644 "$udir/urc-agent-health.timer" /etc/systemd/system/urc-agent-health.timer
  install -Dm644 "$udir/urc-coordinator-health.service" /etc/systemd/system/urc-coordinator-health.service
  install -Dm644 "$udir/urc-coordinator-health.timer" /etc/systemd/system/urc-coordinator-health.timer
  install -Dm644 "$udir/urc-agent-login.path" /etc/systemd/system/urc-agent-login.path
  install -Dm644 "$udir/urc-agent-login.service" /etc/systemd/system/urc-agent-login.service
  systemctl daemon-reload
}

enable_agent() {
  systemctl enable --now urc-agent.service
  systemctl enable --now urc-agent-health.timer
  systemctl enable urc-agent-login.path
}

enable_coordinator() {
  systemctl enable --now urc-coordinator.service
  systemctl enable --now urc-coordinator-health.timer
}

print_finish_coordinator() {
  local ip
  ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
  save_credentials "${ip:-YOUR_VPS_IP}"
  echo ""
  echo "============================================"
  echo "  Coordinator ready on port 21150"
  echo "============================================"
  echo ""
  echo "  1. Open firewall: TCP 21150"
  echo "  2. On each PC to control, run:"
  echo "       sudo ./install --role agent \\"
  echo "         --coordinator-url ws://${ip:-YOUR_VPS_IP}:21150/ws/agent \\"
  echo "         --token $(cat /etc/urc/token) -y"
  echo ""
  echo "  Credentials saved: $CREDENTIALS_FILE"
  echo ""
}

print_finish_agent() {
  local hid="${HOST_ID:-$(hostname -s)}"
  local coord_base="${COORDINATOR_URL%/ws/agent}"
  coord_base="${coord_base%/ws/client}"
  save_credentials ""
  echo ""
  echo "============================================"
  echo "  This PC is ready: host id = $hid"
  echo "============================================"
  echo ""
  echo "  From your laptop (after ./install --role client):"
  echo "    urc connect $hid"
  echo ""
  echo "  VNC + API credentials: $CREDENTIALS_FILE"
  echo "  Health: urc-agent status"
  echo ""
  if [[ "$PROFILE" == "desktop" ]]; then
    echo "  Reboot once to start the graphical session."
    echo ""
  fi
}

print_finish_client() {
  write_client_config "$COORDINATOR_URL" "$HOST_ID"
  echo ""
  echo "============================================"
  echo "  Client ready"
  echo "============================================"
  echo ""
  if [[ -n "$HOST_ID" ]]; then
    echo "  Connect now:"
    echo "    urc connect $HOST_ID"
  else
    echo "  Connect:"
    echo "    urc connect HOST_ID"
  fi
  echo ""
}

# --- main ---

if [[ -z "$ROLE" ]]; then
  run_interactive
fi

if [[ -z "$ROLE" ]]; then
  echo "No role selected." >&2
  exit 1
fi

if [[ "$ROLE" != "client" ]] && [[ "$(uname -s)" != "Linux" ]]; then
  echo "Agent and coordinator require Linux." >&2
  exit 1
fi

if $NONINTERACTIVE; then
  case "$ROLE" in
    agent)
      [[ -n "$COORDINATOR_URL" ]] || {
        echo "Non-interactive agent install requires --coordinator-url ws://VPS:21150/ws/agent" >&2
        exit 1
      }
      if [[ -z "$PROFILE" ]]; then
        if has_desktop; then PROFILE=minimal; else PROFILE=desktop; fi
      fi
      ;;
    client)
      [[ -n "$COORDINATOR_URL" ]] || {
        echo "Non-interactive client install requires --coordinator-url ws://VPS:21150/ws/client" >&2
        exit 1
      }
      ;;
  esac
fi

configure_role_interactive

case "$ROLE" in
  coordinator)
    PROFILE=coordinator
    ensure_token
    build_and_install_binaries
    setup_coordinator_config
    install_libexec
    install_systemd_units
    enable_coordinator
    print_finish_coordinator
    ;;
  agent)
    [[ -z "$PROFILE" ]] && PROFILE=minimal
    ensure_token
  [[ -z "$HOST_ID" ]] && HOST_ID="$(hostname -s)"
    if [[ "$PROFILE" == "desktop" ]]; then
      install_agent_deps
      install_desktop_deps
    else
      install_agent_deps
    fi
    build_and_install_binaries
    setup_agent_config
    install_libexec
    install_systemd_units
    enable_agent
    print_finish_agent
    ;;
  client)
    ensure_token
    install_client_deps
    build_and_install_binaries
    [[ -z "$COORDINATOR_URL" ]] && COORDINATOR_URL="ws://127.0.0.1:21150/ws/client"
    print_finish_client
    ;;
  *)
    echo "Unknown role: $ROLE" >&2
    exit 1
    ;;
esac

if $WITH_TAILSCALE; then
  command -v tailscale >/dev/null 2>&1 || curl -fsSL https://tailscale.com/install.sh | sh
  echo "Tip: set [tailscale] enabled = true in /etc/urc/agent.toml"
fi
