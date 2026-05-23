#!/bin/sh
# macOS App Store Tailscale crashes when invoked via symlink (argv[0] must not be a link).
exec /Applications/Tailscale.app/Contents/MacOS/Tailscale "$@"
