#!/usr/bin/env bash
# One command: build, deploy, fix permissions, start remote desktop.
set -euo pipefail
cd "$(dirname "$0")"
if [[ "$(id -u)" -ne 0 ]]; then
  exec sudo -E bash "$0" "$@"
fi
export PATH="${HOME}/.cargo/bin:/usr/local/bin:$PATH"
if [[ -n "${SUDO_USER:-}" ]] && [[ -f "/home/${SUDO_USER}/.cargo/env" ]]; then
  # shellcheck source=/dev/null
  source "/home/${SUDO_USER}/.cargo/env"
fi
build_user="${SUDO_USER:-$USER}"
echo "==> Building urc-agent (as ${build_user})"
sudo -u "$build_user" bash -lc 'cd "'"$PWD"'" && source "$HOME/.cargo/env" 2>/dev/null; cargo build --release -p urc-agent'
echo "==> Deploying and starting"
exec bash ./packaging/scripts/urc-recover-x11.sh
