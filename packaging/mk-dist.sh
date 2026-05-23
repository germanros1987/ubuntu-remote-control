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

Example (after uploading to your server):

  curl -fsSL https://YOUR_SERVER/install | sudo bash

  # Or prebuilt (no Rust on target):
  curl -fsSL https://YOUR_SERVER/install | sudo URC_BINARIES_URL=https://YOUR_SERVER/linux-${ARCH} URC_RAW_URL=https://YOUR_SERVER/raw/main bash

  # Or tarball only:
  curl -fsSL https://YOUR_SERVER/install | sudo URC_SOURCE_TARBALL=https://YOUR_SERVER/urc-source.tar.gz bash
EOF
