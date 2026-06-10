#!/usr/bin/env bash
# Download the OFFICIAL rclone binary and stage it as a Tauri sidecar.
#
# Why: Homebrew's rclone is built WITHOUT FUSE-mount support on macOS (brew
# can't depend on the macFUSE cask), so `rclone mount` hard-fails with
# "rclone mount is not supported on MacOS when rclone is installed via
# Homebrew". Bundling the official build removes the install step entirely —
# the app always uses its own known-good rclone.
#
# Output: src-tauri/binaries/rclone-<target-triple>  (referenced by
# tauri.conf.json bundle.externalBin; Tauri ships it as Contents/MacOS/rclone)

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST_DIR="$ROOT/src-tauri/binaries"
TRIPLE="${1:-aarch64-apple-darwin}"

case "$TRIPLE" in
  aarch64-apple-darwin) ZIP="rclone-current-osx-arm64.zip" ;;
  x86_64-apple-darwin)  ZIP="rclone-current-osx-amd64.zip" ;;
  *) echo "unsupported triple: $TRIPLE" >&2; exit 1 ;;
esac

DEST="$DEST_DIR/rclone-$TRIPLE"
if [ -x "$DEST" ]; then
  echo "✓ rclone sidecar already present: $DEST ($("$DEST" --version 2>/dev/null | head -1))"
  exit 0
fi

mkdir -p "$DEST_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading official rclone ($ZIP)…"
curl -fsSL "https://downloads.rclone.org/$ZIP" -o "$TMP/rclone.zip"
unzip -q "$TMP/rclone.zip" -d "$TMP"
BIN="$(find "$TMP" -name rclone -type f | head -1)"
[ -n "$BIN" ] || { echo "rclone binary not found in zip" >&2; exit 1; }

cp "$BIN" "$DEST"
chmod +x "$DEST"
echo "✓ Staged $DEST ($("$DEST" --version | head -1))"
