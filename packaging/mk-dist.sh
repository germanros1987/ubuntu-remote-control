#!/usr/bin/env bash
# Build release artifacts for curl-based install (no git on user machines).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST="$REPO_ROOT/dist"
ARCH="$(uname -m)"
OUT="$DIST/linux-${ARCH}"

echo "==> Building release binaries"
cd "$REPO_ROOT"
cargo build --release

mkdir -p "$OUT" "$DIST"
install -Dm755 target/release/urc-agent "$OUT/urc-agent"
install -Dm755 target/release/urc-coordinator "$OUT/urc-coordinator"
install -Dm755 target/release/urc-client "$OUT/urc-client"
install -Dm755 packaging/scripts/urc "$OUT/urc"

echo "==> Creating source tarball (curl install)"
tar czf "$DIST/urc-source.tar.gz" \
  --exclude=target --exclude=dist --exclude=.git \
  -C "$REPO_ROOT" .

cp "$REPO_ROOT/install" "$DIST/install"

cat <<EOF

Done. Host these files on any HTTPS server:

  dist/install              # curl | sudo bash entrypoint
  dist/urc-source.tar.gz    # URC_SOURCE_TARBALL=.../urc-source.tar.gz
  dist/linux-${ARCH}/       # URC_BINARIES_URL=.../linux-${ARCH}

Install from GitHub (public repo — no upload needed):

  curl -fsSL https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main/install | sudo bash

Self-hosted mirror (after uploading dist/ to your HTTPS server):

  curl -fsSL https://your-server/install | sudo bash

  # Prebuilt binaries (no Rust on target):
  curl -fsSL https://your-server/install | sudo \\
    URC_BINARIES_URL=https://your-server/linux-${ARCH} \\
    URC_RAW_URL=https://raw.githubusercontent.com/germanros1987/ubuntu-remote-control/main bash

  # Tarball only:
  curl -fsSL https://your-server/install | sudo \\
    URC_SOURCE_TARBALL=https://your-server/urc-source.tar.gz bash
EOF
