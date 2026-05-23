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
WITHOUT_TAILSCALE=false
TAILSCALE_AUTH_KEY="${URC_TAILSCALE_AUTH_KEY:-}"
NONINTERACTIVE=false
INSTALL_PREFIX="/usr/local"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CREDENTIALS_FILE="/etc/urc/credentials"

usage() {
  cat <<'EOF'
Ubuntu Remote Control — installer (Tailscale turn-key)

  sudo ./install                     Interactive: PC to control or laptop client
  sudo ./install --role agent        Remote-into this machine (+ Tailscale)
  sudo ./install --role client       Laptop — list/connect via Tailscale
  sudo ./install --role coordinator  Optional VPS relay (advanced)

Options:
  --coordinator-url URL   Optional relay (omit for Tailscale-only)
  --host-id NAME          Tailscale / urc name (default: hostname)
  --without-tailscale     Skip Tailscale (not recommended)
  --tailscale-auth-key K  Unattended tailscale up (or URC_TAILSCALE_AUTH_KEY)
  -y, --yes               Non-interactive

After install (same Tailscale account on all machines):
  urc hosts
  urc connect NAME
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
    --without-tailscale) WITHOUT_TAILSCALE=true; shift ;;
    --tailscale-auth-key) TAILSCALE_AUTH_KEY="$2"; shift 2 ;;
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

want_tailscale() {
  $WITHOUT_TAILSCALE && return 1
  $WITH_TAILSCALE && return 0
  [[ "$ROLE" == "agent" || "$ROLE" == "client" ]]
}

tailscale_mode() {
  want_tailscale && [[ -z "${COORDINATOR_URL:-}" ]]
}

setup_tailscale() {
  echo "==> Installing Tailscale"
  if ! command -v tailscale >/dev/null 2>&1; then
    curl -fsSL https://tailscale.com/install.sh | sh
  fi
  systemctl enable --now tailscaled 2>/dev/null || true

  if ! tailscale status --json 2>/dev/null | grep -qE '"BackendState":"Running"|"State":10'; then
    if [[ -n "$TAILSCALE_AUTH_KEY" ]]; then
      tailscale up --auth-key="$TAILSCALE_AUTH_KEY" --accept-routes
    elif ! $NONINTERACTIVE; then
      echo "Sign in to Tailscale (open the URL below if prompted):"
      tailscale up || true
    else
      echo "Tailscale installed. Finish login with: sudo tailscale up"
      echo "  (or pass --tailscale-auth-key / set URC_TAILSCALE_AUTH_KEY)"
    fi
  else
    echo "Tailscale already connected."
  fi

  local hn="${HOST_ID:-$(hostname -s)}"
  tailscale set --hostname="$hn" 2>/dev/null || true
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
  echo "  1) PC I remote INTO (install agent + Tailscale)"
  echo "  2) Laptop I connect FROM (install client + Tailscale)"
  echo "  3) Advanced: VPS relay server (optional)"
  echo ""
  local choice
  choice="$(prompt "Choose 1-3" "1")"
  case "$choice" in
    1) ROLE=agent ;;
    2) ROLE=client ;;
    3) ROLE=coordinator ;;
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
        echo "Desktop detected — agent + Tailscale (no VPS needed)."
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
      HOST_ID="$(prompt "Name for this PC (shown in urc hosts)" "$(hostname -s)")"
      if ! $NONINTERACTIVE && want_tailscale; then
        local relay
        relay="$(prompt "Use a VPS relay instead of Tailscale-only? [y/N]" "N")"
        if [[ "$relay" =~ ^[Yy] ]]; then
          COORDINATOR_URL="$(prompt "Coordinator WebSocket URL" "ws://YOUR_VPS_IP:21150/ws/agent")"
        fi
      fi
      ;;
    client)
      echo "Client + Tailscale — you will see all PCs on your tailnet."
      if ! $NONINTERACTIVE; then
        local relay
        relay="$(prompt "Use a VPS relay instead of Tailscale-only? [y/N]" "N")"
        if [[ "$relay" =~ ^[Yy] ]]; then
          COORDINATOR_URL="$(prompt "Coordinator WebSocket URL" "ws://YOUR_VPS_IP:21150/ws/client")"
        fi
      fi
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
  if [[ -n "${COORDINATOR_URL:-}" ]]; then
    sed -i "s|^coordinator_url = .*|coordinator_url = \"$COORDINATOR_URL\"|" /etc/urc/agent.toml
  else
    sed -i 's|^coordinator_url = .*|coordinator_url = ""|' /etc/urc/agent.toml
  fi
  if want_tailscale; then
    sed -i '/^\[tailscale\]/,/^\[/ s/^enabled = .*/enabled = true/' /etc/urc/agent.toml
  fi
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
  save_credentials ""
  echo ""
  echo "============================================"
  echo "  This PC is ready on Tailscale as: $hid"
  echo "============================================"
  echo ""
  echo "  On your laptop (same Tailscale account):"
  echo "    urc hosts"
  echo "    urc connect $hid"
  echo ""
  echo "  VNC password (if generated): $CREDENTIALS_FILE"
  echo "  Health: urc-agent status"
  if want_tailscale && command -v tailscale >/dev/null 2>&1; then
    local ts_ip
    ts_ip="$(tailscale ip -4 2>/dev/null || true)"
    if [[ -n "$ts_ip" ]]; then
      echo "  Tailscale: $ts_ip (direct TLS on port 15900 when logged in)"
    else
      echo "  Tailscale: run 'sudo tailscale up' to finish login"
    fi
  fi
  echo ""
  if [[ "$PROFILE" == "desktop" ]]; then
    echo "  Reboot once to start the graphical session."
    echo ""
  fi
}

print_finish_client() {
  write_client_config "${COORDINATOR_URL:-}" "$HOST_ID"
  echo ""
  echo "============================================"
  echo "  Client ready"
  echo "============================================"
  echo ""
  echo "  List your PCs:"
  echo "    urc hosts"
  echo ""
  echo "  Connect:"
  echo "    urc connect NAME"
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
      if [[ -z "$PROFILE" ]]; then
        if has_desktop; then PROFILE=minimal; else PROFILE=desktop; fi
      fi
      ;;
    client) ;;
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
    if want_tailscale; then
      setup_tailscale
    fi
    install_libexec
    install_systemd_units
    enable_agent
    print_finish_agent
    ;;
  client)
    ensure_token
    install_client_deps
    build_and_install_binaries
    if want_tailscale; then
      setup_tailscale
    fi
    print_finish_client
    ;;
  *)
    echo "Unknown role: $ROLE" >&2
    exit 1
    ;;
esac

