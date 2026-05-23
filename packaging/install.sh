#!/usr/bin/env bash
# Ubuntu Remote Control — one-command installer
set -euo pipefail

PROFILE="minimal"
GPU="auto"
TOKEN=""
COORDINATOR_URL=""
WITH_TAILSCALE=false
INSTALL_PREFIX="/usr/local"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<EOF
Usage: install.sh [options]

  --profile minimal|desktop|coordinator  (default: minimal)
  --gpu auto|nvidia|intel|amd            (desktop profile only)
  --token SECRET                         coordinator/agent auth token
  --coordinator-url URL                  e.g. ws://vps.example.com:21150/ws/agent
  --with-tailscale                       install & recommend tailscale
  --prefix PATH                          (default: /usr/local)

Profiles:
  minimal      agent + VNC tools only (machine must already have a GUI session)
  desktop      full GPU desktop stack for headless Ubuntu Server
  coordinator  rendezvous + relay only (for VPS)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile) PROFILE="$2"; shift 2 ;;
    --gpu) GPU="$2"; shift 2 ;;
    --token) TOKEN="$2"; shift 2 ;;
    --coordinator-url) COORDINATOR_URL="$2"; shift 2 ;;
    --with-tailscale) WITH_TAILSCALE=true; shift ;;
    --prefix) INSTALL_PREFIX="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1"; usage; exit 1 ;;
  esac
done

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Run as root: sudo bash install.sh ..." >&2
  exit 1
fi

has_desktop() {
  dpkg -l ubuntu-desktop-minimal 2>/dev/null | grep -q '^ii' || \
  dpkg -l ubuntu-desktop 2>/dev/null | grep -q '^ii' || \
  dpkg -l gnome-shell 2>/dev/null | grep -q '^ii'
}

if [[ "$PROFILE" == "minimal" ]] && ! has_desktop; then
  echo "WARNING: No Ubuntu desktop detected."
  echo "  Remote GUI will NOT work until a graphical session exists."
  echo "  For headless servers use: --profile desktop --gpu auto"
  echo ""
  read -r -p "Continue with minimal install? [y/N] " ans || true
  [[ "${ans:-}" =~ ^[Yy]$ ]] || exit 1
fi

echo "==> Installing base packages"
apt-get update -qq
apt-get install -y -qq curl ca-certificates

install_minimal_deps() {
  apt-get install -y -qq tigervnc-standalone-server tigervnc-common
}

install_desktop_deps() {
  apt-get install -y -qq \
    xserver-xorg xserver-xorg-video-all \
    lightdm ubuntu-desktop-minimal \
    gnome-remote-desktop

  case "$GPU" in
    nvidia)
      apt-get install -y -qq nvidia-driver-535 || apt-get install -y -qq nvidia-driver-550
      ;;
    intel)
      apt-get install -y -qq intel-media-va-driver mesa-va-drivers vainfo
      ;;
    amd)
      apt-get install -y -qq mesa-vulkan-drivers mesa-va-drivers
      ;;
    auto)
      if lspci 2>/dev/null | grep -qi nvidia; then
        apt-get install -y -qq nvidia-driver-535 || true
      fi
      ;;
  esac

  REMOTE_USER="${SUDO_USER:-ubuntu}"
  if [[ -f /etc/lightdm/lightdm.conf ]]; then
    if ! grep -q '^autologin-user=' /etc/lightdm/lightdm.conf; then
      sed -i "/^\[Seat:\*\]/a autologin-user=$REMOTE_USER\nautologin-user-timeout=0" /etc/lightdm/lightdm.conf || true
    fi
  fi
  systemctl enable lightdm || true
}

install_coordinator_deps() {
  : # coordinator is a static binary
}

build_and_install_binaries() {
  if ! command -v cargo >/dev/null 2>&1; then
    apt-get install -y -qq cargo rustc
  fi
  cd "$REPO_ROOT"
  cargo build --release
  install -Dm755 target/release/urc-agent "$INSTALL_PREFIX/bin/urc-agent"
  install -Dm755 target/release/urc-client "$INSTALL_PREFIX/bin/urc-client"
  install -Dm755 target/release/urc-coordinator "$INSTALL_PREFIX/bin/urc-coordinator"
}

setup_config() {
  install -d -m755 /etc/urc
  if [[ ! -f /etc/urc/agent.toml ]]; then
    "$INSTALL_PREFIX/bin/urc-agent" --init-config > /etc/urc/agent.toml
  fi
  if [[ -n "$TOKEN" ]]; then
    sed -i "s/^token = .*/token = \"$TOKEN\"/" /etc/urc/agent.toml 2>/dev/null || \
      echo "token = \"$TOKEN\"" >> /etc/urc/agent.toml
  fi
  if [[ -n "$COORDINATOR_URL" ]]; then
    sed -i "s|^coordinator_url = .*|coordinator_url = \"$COORDINATOR_URL\"|" /etc/urc/agent.toml 2>/dev/null || \
      echo "coordinator_url = \"$COORDINATOR_URL\"" >> /etc/urc/agent.toml
  fi
  install -d -m700 /etc/urc/tls
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

enable_agent_stack() {
  systemctl enable --now urc-agent.service
  systemctl enable --now urc-agent-health.timer
  systemctl enable urc-agent-login.path
}

enable_coordinator_stack() {
  systemctl enable --now urc-coordinator.service
  systemctl enable --now urc-coordinator-health.timer
}

case "$PROFILE" in
  minimal)
    install_minimal_deps
    build_and_install_binaries
    setup_config
    install_libexec
    install_systemd_units
    enable_agent_stack
    ;;
  desktop)
    install_minimal_deps
    install_desktop_deps
    build_and_install_binaries
    setup_config
    install_libexec
    install_systemd_units
    enable_agent_stack
    ;;
  coordinator)
    install_coordinator_deps
    build_and_install_binaries
    install_libexec
    install_systemd_units
    enable_coordinator_stack
    ;;
  *)
    echo "Unknown profile: $PROFILE" >&2
    exit 1
    ;;
esac

if $WITH_TAILSCALE; then
  if ! command -v tailscale >/dev/null 2>&1; then
    curl -fsSL https://tailscale.com/install.sh | sh
  fi
  echo "Enable Tailscale in /etc/urc/agent.toml: [tailscale] enabled = true"
fi

echo ""
echo "URC installed (profile=$PROFILE)."
echo "  Agent config:     /etc/urc/agent.toml"
echo "  Health timer:     systemctl status urc-agent-health.timer"
echo "  Client:           urc-client --token TOKEN connect HOST_ID"
