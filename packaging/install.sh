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
OS="$(uname -s)"

is_macos() { [[ "$OS" == "Darwin" ]]; }
is_linux() { [[ "$OS" == "Linux" ]]; }

if is_macos; then
  URC_ETC="${URC_ETC:-/usr/local/etc/urc}"
else
  URC_ETC="${URC_ETC:-/etc/urc}"
fi
CREDENTIALS_FILE="$URC_ETC/credentials"
CLIENT_ENV_FILE="$URC_ETC/client.env"

usage() {
  cat <<'EOF'
Ubuntu Remote Control — installer (Tailscale turn-key)

  sudo ./install                     Interactive: PC to control or laptop client
  sudo ./install --role agent        Remote-into this machine (+ Tailscale, Linux only)
  sudo ./install --role client       Laptop/Mac — list/connect via Tailscale
  sudo ./install --role coordinator  Optional VPS relay (Linux only)

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

# Read from the terminal (works with: curl ... | sudo bash)
prompt() {
  local msg="$1" default="${2:-}"
  if $NONINTERACTIVE; then
    echo "${default}"
    return
  fi
  if [[ ! -r /dev/tty ]]; then
    echo "ERROR: Interactive install needs a terminal (stdin is not your keyboard)." >&2
    echo "  Save and run: curl -fsSL ... -o /tmp/urc-install.sh && sudo bash /tmp/urc-install.sh" >&2
    echo "  Or pass a role:  curl ... | sudo bash -s -- --role agent|client|coordinator" >&2
    exit 1
  fi
  local ans=""
  if [[ -n "$default" ]]; then
    printf '%s [%s]: ' "$msg" "$default" >/dev/tty
    read -r ans </dev/tty || true
    echo "${ans:-$default}"
  else
    printf '%s: ' "$msg" >/dev/tty
    read -r ans </dev/tty || true
    echo "$ans"
  fi
}

sed_inplace() {
  if is_macos; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

has_desktop() {
  is_linux || return 1
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

  local ts_user="${SUDO_USER:-${USER:-root}}"
  local hn="${HOST_ID:-$(hostname -s)}"

  if ! tailscale status --json 2>/dev/null | grep -qE '"BackendState":"Running"|"State":10'; then
    if [[ -n "$TAILSCALE_AUTH_KEY" ]]; then
      if is_macos && [[ "$ts_user" != "root" ]]; then
        sudo -u "$ts_user" tailscale up --auth-key="$TAILSCALE_AUTH_KEY" --accept-routes
      else
        tailscale up --auth-key="$TAILSCALE_AUTH_KEY" --accept-routes
      fi
    elif ! $NONINTERACTIVE && [[ -r /dev/tty ]]; then
      echo "Sign in to Tailscale (open the URL below if prompted):" >/dev/tty
      if is_macos && [[ "$ts_user" != "root" ]]; then
        sudo -u "$ts_user" tailscale up </dev/tty >/dev/tty 2>&1 || true
      else
        tailscale up </dev/tty >/dev/tty 2>&1 || true
      fi
    else
      echo "Tailscale installed. Finish login with: tailscale up"
      echo "  (or pass --tailscale-auth-key / set URC_TAILSCALE_AUTH_KEY)"
    fi
  else
    echo "Tailscale already connected."
  fi

  if is_macos && [[ "$ts_user" != "root" ]]; then
    sudo -u "$ts_user" tailscale set --hostname="$hn" 2>/dev/null || true
  else
    tailscale set --hostname="$hn" 2>/dev/null || true
  fi
}

ensure_token() {
  install -d -m755 "$URC_ETC"
  if [[ -n "$TOKEN" ]]; then
    return
  fi
  if [[ -f "$URC_ETC/token" ]]; then
    TOKEN="$(cat "$URC_ETC/token")"
    return
  fi
  TOKEN="$(openssl rand -hex 24)"
  echo "$TOKEN" > "$URC_ETC/token"
  chmod 600 "$URC_ETC/token"
}

save_credentials() {
  local coord_pub="${1:-}"
  install -d -m755 "$URC_ETC"
  cat > "$CREDENTIALS_FILE" <<EOF
# Ubuntu Remote Control — save these somewhere safe
URC_TOKEN=$TOKEN
URC_COORDINATOR_PUBLIC=$coord_pub
URC_HOST_ID=${HOST_ID:-$(hostname -s)}
EOF
  if [[ -n "${URC_VNC_PASSWORD_PLAIN:-}" ]]; then
    echo "URC_VNC_PASSWORD=$URC_VNC_PASSWORD_PLAIN" >> "$CREDENTIALS_FILE"
  fi
  chmod 600 "$CREDENTIALS_FILE"
}

write_client_config() {
  local coord_client="$1"
  local target_host="${2:-}"
  install -d -m755 "$URC_ETC"
  cat > "$CLIENT_ENV_FILE" <<EOF
URC_COORDINATOR=$coord_client
URC_TOKEN=$TOKEN
URC_DEFAULT_HOST=$target_host
EOF
  chmod 644 "$CLIENT_ENV_FILE"
}

run_interactive() {
  echo ""
  echo "=== Ubuntu Remote Control ==="
  echo ""
  if is_macos; then
    echo "macOS detected — installing the client (connect FROM this Mac)."
    echo "Use role 1 on each Ubuntu PC you want to control."
    echo ""
    ROLE=client
    return
  fi
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

mac_build_user() {
  echo "${SUDO_USER:-${USER:-root}}"
}

mac_run_user() {
  local build_user
  build_user="$(mac_build_user)"
  if [[ "$build_user" == "root" ]]; then
    return 1
  fi
  sudo -u "$build_user" -H bash -lc "$1"
}

mac_has_cargo() {
  mac_run_user 'source "$HOME/.cargo/env" 2>/dev/null; cargo --version' >/dev/null 2>&1
}

install_base_packages() {
  echo "==> Installing system packages"
  if is_macos; then
    if ! command -v curl >/dev/null 2>&1; then
      echo "ERROR: curl is required. Install Xcode Command Line Tools: xcode-select --install" >&2
      exit 1
    fi
    return
  fi
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq curl ca-certificates openssl
}

install_build_deps() {
  if is_macos; then
    if mac_has_cargo; then
      return
    fi
    echo "==> Rust not found — install via rustup (one-time)"
    mac_run_user 'curl -fsSL https://sh.rustup.rs | sh -s -- -y -q' || true
    mac_run_user 'source "$HOME/.cargo/env" && rustup default stable' 2>/dev/null || true
    if mac_has_cargo; then
      return
    fi
    local build_user home
    build_user="$(mac_build_user)"
    home="$(eval echo "~${build_user}")"
    if [[ -x "${home}/.cargo/bin/cargo" ]]; then
      return
    fi
    echo "ERROR: Rust installed but cargo not in PATH." >&2
    echo "  Run:  source \"\$HOME/.cargo/env\"  &&  re-run the installer" >&2
    exit 1
  fi
  # rustup shim without a default toolchain breaks `cargo build`
  if command -v rustup >/dev/null 2>&1; then
    echo "==> Configuring Rust toolchain"
    if [[ -n "${SUDO_USER:-}" ]] && [[ "$(id -u)" -eq 0 ]]; then
      sudo -u "$SUDO_USER" -H rustup default stable 2>/dev/null || true
    else
      rustup default stable 2>/dev/null || true
    fi
  fi
  if ! cargo --version >/dev/null 2>&1; then
    apt-get install -y -qq cargo rustc build-essential pkg-config
  fi
  if ! cargo --version >/dev/null 2>&1 && [[ -x /usr/bin/cargo ]]; then
    export PATH="/usr/bin:/usr/sbin:$PATH"
  fi
  if ! cargo --version >/dev/null 2>&1; then
    echo "ERROR: Could not run cargo. Try: sudo apt install cargo rustc  OR  rustup default stable" >&2
    exit 1
  fi
}

install_client_binaries() {
  if [[ -n "${URC_SKIP_BUILD:-}" ]] && [[ -n "${URC_BIN_DIR:-}" ]]; then
    echo "==> Installing prebuilt client"
    install -Dm755 "$URC_BIN_DIR/urc-client" "$INSTALL_PREFIX/bin/urc-client"
    install -Dm755 "$URC_BIN_DIR/urc" "$INSTALL_PREFIX/bin/urc"
    return
  fi
  install_build_deps
  echo "==> Building URC client (first install — may take a few minutes)"
  if is_macos; then
    mac_run_user "cd '$REPO_ROOT' && source \"\$HOME/.cargo/env\" && cargo build --release -p urc-client"
  else
    cd "$REPO_ROOT"
    cargo build --release -p urc-client
  fi
  install -Dm755 "$REPO_ROOT/target/release/urc-client" "$INSTALL_PREFIX/bin/urc-client"
  install -Dm755 "$REPO_ROOT/packaging/scripts/urc" "$INSTALL_PREFIX/bin/urc"
}

build_and_install_binaries() {
  if is_macos; then
    install_client_binaries
    return
  fi
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
    sed_inplace "/^\[Seat:\*\]/a autologin-user=$u\nautologin-user-timeout=0" /etc/lightdm/lightdm.conf || true
  fi
  systemctl enable lightdm 2>/dev/null || true
}

install_client_deps() {
  if is_macos; then
    echo "==> macOS client dependencies"
    if command -v brew >/dev/null 2>&1; then
      if ! command -v vncviewer >/dev/null 2>&1; then
        echo "==> Installing TigerVNC Viewer (Homebrew)"
        brew install --cask tigervnc-viewer 2>/dev/null || brew install tigervnc-viewer 2>/dev/null || true
      fi
    fi
    if ! command -v vncviewer >/dev/null 2>&1; then
      echo "Tip: install a VNC viewer: brew install --cask tigervnc-viewer" >&2
    fi
    return
  fi
  apt-get install -y -qq tigervnc-viewer || true
}

setup_vnc_password() {
  if [[ -f /etc/urc/vncpasswd ]]; then
    return
  fi
  local vnc_pass vncpwd
  vnc_pass="$(openssl rand -base64 12 | tr -d '/+=' | head -c 12)"
  # At least 6 chars for TigerVNC; only first 8 matter.
  [[ ${#vnc_pass} -lt 6 ]] && vnc_pass="${vnc_pass}abcdef"
  install -d -m755 /etc/urc
  vncpwd="$(command -v vncpasswd || command -v tigervncpasswd || true)"
  if [[ -z "$vncpwd" ]]; then
    echo "Note: install tigervnc, then: printf 'PASSWORD\\n' | vncpasswd -f > /etc/urc/vncpasswd" >&2
    return
  fi
  if printf '%s\n' "$vnc_pass" | "$vncpwd" -f > /etc/urc/vncpasswd 2>/dev/null; then
    chmod 600 /etc/urc/vncpasswd
    export URC_VNC_PASSWORD_PLAIN="$vnc_pass"
  else
    echo "Note: create VNC password manually:" >&2
    echo "  printf 'YOUR_PASSWORD\\n' | vncpasswd -f | sudo tee /etc/urc/vncpasswd >/dev/null && sudo chmod 600 /etc/urc/vncpasswd" >&2
  fi
}

setup_agent_config() {
  install -d -m755 /etc/urc
  "$INSTALL_PREFIX/bin/urc-agent" --init-config > /etc/urc/agent.toml
  local hid="${HOST_ID:-$(hostname -s)}"
  sed_inplace "s/^host_id = .*/host_id = \"$hid\"/" /etc/urc/agent.toml
  sed_inplace "s/^token = .*/token = \"$TOKEN\"/" /etc/urc/agent.toml
  if [[ -n "${COORDINATOR_URL:-}" ]]; then
    sed_inplace "s|^coordinator_url = .*|coordinator_url = \"$COORDINATOR_URL\"|" /etc/urc/agent.toml
  else
    sed_inplace 's|^coordinator_url = .*|coordinator_url = ""|' /etc/urc/agent.toml
  fi
  if want_tailscale; then
    sed_inplace '/^\[tailscale\]/,/^\[/ s/^enabled = .*/enabled = true/' /etc/urc/agent.toml
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
  if is_macos; then
    echo "  Config: $CLIENT_ENV_FILE"
    echo "  (TigerVNC: brew install --cask tigervnc-viewer if needed)"
    echo ""
  fi
  echo "  List your PCs:"
  echo "    urc hosts"
  echo ""
  echo "  Connect:"
  echo "    urc connect NAME"
  echo ""
}

# --- main ---

install_base_packages

if [[ -z "$ROLE" ]]; then
  run_interactive
fi

if [[ -z "$ROLE" ]]; then
  echo "No role selected." >&2
  exit 1
fi

if [[ "$ROLE" != "client" ]] && ! is_linux; then
  echo "On macOS only the client (laptop) role is supported." >&2
  echo "Install role agent on each Ubuntu PC you want to control." >&2
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

