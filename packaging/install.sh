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

# Do not use macOS/BSD install(1) — it breaks with GNU-style -D (INS@ temp files).
install_file() {
  local mode="$1" src="$2" dest="$3"
  if [[ ! -e "$src" ]]; then
    echo "ERROR: missing install source: $src" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$dest")"
  cp -f "$src" "$dest"
  chmod "$mode" "$dest"
}

# Run a long step quietly: write its output to a log file, show a spinner with
# the elapsed seconds, and on failure spill the last 40 lines of the log so the
# user has something to act on. Keeps the installer banner clean.
quiet_step() {
  local label="$1"; shift
  local log; log="$(mktemp /tmp/urc-step.XXXXXX.log)"
  local start=$SECONDS
  printf '==> %s ... ' "$label"
  local rc=0
  "$@" >"$log" 2>&1 || rc=$?
  local dt=$((SECONDS - start))
  if [[ $rc -eq 0 ]]; then
    printf '✓ (%ds)\n' "$dt"
    rm -f "$log"
    return 0
  fi
  printf '✗ (%ds)\n' "$dt"
  echo "--- last 40 lines of $log ---" >&2
  tail -40 "$log" >&2
  return $rc
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
  if ! command -v tailscale >/dev/null 2>&1; then
    quiet_step "Installing Tailscale" \
      bash -c 'curl -fsSL https://tailscale.com/install.sh | sh'
  else
    echo "==> Tailscale already installed"
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
    elif [[ -r /dev/tty ]]; then
      # Tailscale login is inherently interactive (it prints a URL to open in a
      # browser), so prompt even under -y/NONINTERACTIVE when a terminal exists.
      echo "==> Tailscale not logged in — signing you in now." >/dev/tty
      echo "    Open the URL below in a browser to authorize this machine:" >/dev/tty
      if is_macos && [[ "$ts_user" != "root" ]]; then
        sudo -u "$ts_user" tailscale up </dev/tty >/dev/tty 2>&1 || true
      else
        tailscale up </dev/tty >/dev/tty 2>&1 || true
      fi
      if ! tailscale status --json 2>/dev/null | grep -qE '"BackendState":"Running"|"State":10'; then
        echo "Tailscale login not completed. Re-run later with: sudo tailscale up" >/dev/tty
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
  # xclip lets the agent bridge the X11 PRIMARY/CLIPBOARD selections, which the
  # unified web UI relies on for desktop ↔ browser clipboard sync.
  apt-get install -y -qq curl ca-certificates openssl xclip
}

rustc_version_ok() {
  local major minor ver
  if [[ -n "${SUDO_USER:-}" ]] && [[ "$(id -u)" -eq 0 ]]; then
    ver=$(linux_run_cargo 'source "$HOME/.cargo/env" 2>/dev/null; rustc --version' 2>/dev/null || true)
  else
    ver=$(rustc --version 2>/dev/null || true)
  fi
  major=$(printf '%s\n' "$ver" | sed -n 's/rustc \([0-9]*\)\..*/\1/p')
  minor=$(printf '%s\n' "$ver" | sed -n 's/rustc [0-9]*\.\([0-9]*\).*/\1/p')
  [[ -n "$major" && -n "$minor" ]] || return 1
  [[ "$major" -gt 1 ]] && return 0
  [[ "$major" -eq 1 && "$minor" -ge 78 ]]
}

linux_install_rustup() {
  local user="${SUDO_USER:-root}"
  echo "==> Installing Rust via rustup (apt rustc is too old for this project)"
  if [[ "$user" == "root" ]]; then
    curl -fsSL https://sh.rustup.rs | sh -s -- -y -q --default-toolchain stable
    # shellcheck source=/dev/null
    source "${HOME}/.cargo/env"
  else
    sudo -u "$user" -H bash -lc 'curl -fsSL https://sh.rustup.rs | sh -s -- -y -q --default-toolchain stable'
  fi
}

linux_run_cargo() {
  local user="${SUDO_USER:-root}"
  if [[ "$user" != "root" ]]; then
    sudo -u "$user" -H bash -lc "$1"
  else
    bash -lc "$1"
  fi
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
  apt-get install -y -qq build-essential pkg-config libssl-dev 2>/dev/null || \
    apt-get install -y -qq build-essential pkg-config
  if ! rustc_version_ok; then
    linux_install_rustup
  elif ! cargo --version >/dev/null 2>&1; then
    apt-get install -y -qq cargo rustc || true
  fi
  if ! rustc_version_ok; then
    linux_install_rustup
  fi
  if ! linux_run_cargo 'source "$HOME/.cargo/env" 2>/dev/null; cargo --version' >/dev/null 2>&1; then
    echo "ERROR: Could not run a modern cargo (need Rust 1.78+)." >&2
    exit 1
  fi
}

install_client_binaries() {
  if [[ -n "${URC_CLIENT_BINARIES_INSTALLED:-}" ]]; then
    echo "==> Client binaries already installed"
    return
  fi
  if [[ -n "${URC_SKIP_BUILD:-}" ]] && [[ -n "${URC_BIN_DIR:-}" ]]; then
    echo "==> Installing prebuilt client"
    install_file 755 "$URC_BIN_DIR/urc-client" "$INSTALL_PREFIX/bin/urc-client"
    install_file 755 "$URC_BIN_DIR/urc" "$INSTALL_PREFIX/bin/urc"
    return
  fi
  install_build_deps
  if is_macos; then
    quiet_step "Building URC client (first install: 1-3 min)" \
      mac_run_user "cd '$REPO_ROOT' && source \"\$HOME/.cargo/env\" && cargo build --quiet --release -p urc-client"
  else
    quiet_step "Building URC client (first install: 1-3 min)" \
      linux_run_cargo "cd '$REPO_ROOT' && source \"\$HOME/.cargo/env\" 2>/dev/null; cargo build --quiet --release -p urc-client"
  fi
  install_file 755 "$REPO_ROOT/target/release/urc-client" "$INSTALL_PREFIX/bin/urc-client"
  install_file 755 "$REPO_ROOT/packaging/scripts/urc" "$INSTALL_PREFIX/bin/urc"
}

build_and_install_binaries() {
  if is_macos; then
    install_client_binaries
    return
  fi
  if [[ -n "${URC_SKIP_BUILD:-}" ]] && [[ -n "${URC_BIN_DIR:-}" ]]; then
    echo "==> Installing prebuilt binaries"
    install_file 755 "$URC_BIN_DIR/urc-agent" "$INSTALL_PREFIX/bin/urc-agent"
    install_file 755 "$URC_BIN_DIR/urc-client" "$INSTALL_PREFIX/bin/urc-client"
    install_file 755 "$URC_BIN_DIR/urc-coordinator" "$INSTALL_PREFIX/bin/urc-coordinator"
    install_file 755 "$URC_BIN_DIR/urc" "$INSTALL_PREFIX/bin/urc"
    return
  fi
  install_build_deps
  quiet_step "Building URC (first install: 1-3 min)" \
    linux_run_cargo "cd '$REPO_ROOT' && source \"\$HOME/.cargo/env\" 2>/dev/null; cargo build --quiet --release"
  install_file 755 target/release/urc-agent "$INSTALL_PREFIX/bin/urc-agent"
  install_file 755 target/release/urc-client "$INSTALL_PREFIX/bin/urc-client"
  install_file 755 target/release/urc-coordinator "$INSTALL_PREFIX/bin/urc-coordinator"
  install_file 755 "$REPO_ROOT/packaging/scripts/urc" "$INSTALL_PREFIX/bin/urc"
}

require_screen_vnc_server() {
  if command -v x0tigervncserver >/dev/null 2>&1 || command -v x0vncserver >/dev/null 2>&1; then
    return 0
  fi
  echo "ERROR: VNC screen server missing after install (expected x0tigervncserver)." >&2
  echo "  Try: apt install tigervnc-scraping-server tigervnc-common" >&2
  exit 1
}

install_agent_deps() {
  echo "==> Installing VNC (screen sharing on your desktop)"
  # Ubuntu 22.04+ renamed x0vncserver → x0tigervncserver (tigervnc-scraping-server).
  apt-get install -y -qq \
    tigervnc-scraping-server \
    tigervnc-standalone-server \
    tigervnc-common \
    tigervnc-tools
  require_screen_vnc_server
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
    echo "==> macOS client — opens the agent's unified web UI in your default browser"
    return
  fi
  # Linux client opens xdg-open against http://localhost:<port>/ — no native VNC viewer required.
  :
}

setup_vnc_password() {
  local desktop_user="${SUDO_USER:-}"
  if [[ -f /etc/urc/vncpasswd ]]; then
    if [[ -n "$desktop_user" ]]; then
      chown "$desktop_user" /etc/urc/vncpasswd 2>/dev/null || true
      chmod 600 /etc/urc/vncpasswd
    fi
    return
  fi
  local vnc_pass vncpwd
  vnc_pass="$(openssl rand -base64 12 | tr -d '/+=' | head -c 12)"
  # At least 6 chars for TigerVNC; only first 8 matter.
  [[ ${#vnc_pass} -lt 6 ]] && vnc_pass="${vnc_pass}abcdef"
  install -d -m755 /etc/urc
  vncpwd="$(command -v vncpasswd || command -v tigervncpasswd || true)"
  if [[ -z "$vncpwd" ]]; then
    echo "ERROR: vncpasswd not found (tigervnc-common should provide it)." >&2
    exit 1
  fi
  if printf '%s\n' "$vnc_pass" | "$vncpwd" -f > /etc/urc/vncpasswd 2>/dev/null; then
    chmod 600 /etc/urc/vncpasswd
    local desktop_user="${SUDO_USER:-}"
    if [[ -n "$desktop_user" ]]; then
      chown "$desktop_user" /etc/urc/vncpasswd
    fi
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
  install_file 755 "$REPO_ROOT/packaging/scripts/urc-fix-agent-perms.sh" /usr/libexec/urc/urc-fix-agent-perms.sh
  install_file 755 "$REPO_ROOT/packaging/scripts/wait-for-session.sh" /usr/libexec/urc/wait-for-session.sh
  install_file 755 "$REPO_ROOT/packaging/scripts/urc-health-check.sh" /usr/libexec/urc/urc-health-check.sh
  install_file 755 "$REPO_ROOT/packaging/scripts/urc-coordinator-health-check.sh" /usr/libexec/urc/urc-coordinator-health-check.sh
}

install_systemd_units() {
  local udir="$REPO_ROOT/packaging/systemd"
  install_file 644 "$udir/urc-agent.service" /etc/systemd/system/urc-agent.service
  install_file 644 "$udir/urc-coordinator.service" /etc/systemd/system/urc-coordinator.service
  install_file 644 "$udir/urc-agent-health.service" /etc/systemd/system/urc-agent-health.service
  install_file 644 "$udir/urc-agent-health.timer" /etc/systemd/system/urc-agent-health.timer
  install_file 644 "$udir/urc-coordinator-health.service" /etc/systemd/system/urc-coordinator-health.service
  install_file 644 "$udir/urc-coordinator-health.timer" /etc/systemd/system/urc-coordinator-health.timer
  install_file 644 "$udir/urc-agent-login.path" /etc/systemd/system/urc-agent-login.path
  install_file 644 "$udir/urc-agent-login.service" /etc/systemd/system/urc-agent-login.service
  systemctl daemon-reload
}

enable_agent() {
  systemctl enable urc-agent.service
  systemctl enable urc-agent-health.timer
  systemctl enable urc-agent-login.path
}

bootstrap_agent_vnc() {
  echo "==> Starting remote desktop (VNC on 5900, TLS on 15900)…"
  if [[ -x /usr/libexec/urc/urc-fix-agent-perms.sh ]]; then
    /usr/libexec/urc/urc-fix-agent-perms.sh || true
  elif [[ -x "$REPO_ROOT/packaging/scripts/urc-fix-agent-perms.sh" ]]; then
    "$REPO_ROOT/packaging/scripts/urc-fix-agent-perms.sh" || true
  fi
  local attempt
  for attempt in 1 2 3 4 5 6; do
    systemctl restart urc-agent.service
    local i
    for i in $(seq 1 20); do
      if ss -tlnp 2>/dev/null | grep -qE ':15900[[:space:]]'; then
        echo "==> Remote desktop ready (port 15900)"
        return 0
      fi
      sleep 2
    done
    if [[ "$attempt" -lt 6 ]]; then
      echo "==> Still waiting (attempt $attempt/6)…"
      journalctl -u urc-agent -n 2 --no-pager 2>/dev/null | sed 's/^/    /' || true
    fi
  done
  echo "ERROR: Remote desktop did not start on port 15900." >&2
  echo "  Ensure you are logged into the graphical desktop on this machine." >&2
  if [[ -x "$REPO_ROOT/packaging/scripts/urc-recover-x11.sh" ]]; then
    echo "  If logs mention 'Maximum number of clients reached', run:" >&2
    echo "    sudo $REPO_ROOT/packaging/scripts/urc-recover-x11.sh" >&2
  fi
  journalctl -u urc-agent -n 25 --no-pager >&2 || true
  exit 1
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
      echo "  Tailscale: $ts_ip"
    else
      echo "  Tailscale: run 'sudo tailscale up' to finish login"
    fi
  fi
  echo "  Remote desktop: listening on port 15900 (connect with: urc connect $hid)"
  echo ""
  if [[ "$PROFILE" == "desktop" ]]; then
    echo "  Reboot once to start the graphical session, then re-run this installer."
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
    echo "  UI:     unified web app (opens in your default browser)"
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
    systemctl daemon-reload
    enable_agent
    bootstrap_agent_vnc
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

