#!/usr/bin/env bash
# Build a PATCHED rclone and stage it as a Tauri sidecar.
#
# Why patched: rclone's macFUSE mount returns ENOSYS for setxattr, which macOS
# surfaces as EPERM. Finder writes extended attributes (Finder tags, quarantine,
# etc.) when copying a FOLDER, so the EPERM makes Finder abort the whole copy
# with "error code -8062". Our patch (scripts/rclone-xattr.patch) makes the
# cmount filesystem ACCEPT + DISCARD xattr writes (return success), so native
# Finder copy/paste of files AND folders works on a local (macFUSE) mount. The
# attributes aren't persisted on S3 — acceptable for a cloud drive.
#
# Build needs: Go, clang (Xcode CLT), and the FUSE headers. cgofuse compiles
# against fuse.h but dlopen()s libfuse at RUNTIME, so we only need the HEADERS at
# build time (vendored in scripts/macfuse-headers/, copied to the path cgofuse's
# CFLAGS hard-code). No macFUSE install / kernel extension required on the runner.
#
# Output: src-tauri/binaries/rclone-<target-triple>  (referenced by
# tauri.conf.json bundle.externalBin; Tauri ships it as Contents/MacOS/rclone)

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST_DIR="$ROOT/src-tauri/binaries"
TRIPLE="${1:-aarch64-apple-darwin}"
RCLONE_VERSION="v1.74.3"            # pinned: the patch matches this tag's fs.go
PATCH="$ROOT/scripts/rclone-xattr.patch"
HEADERS="$ROOT/scripts/macfuse-headers"

case "$TRIPLE" in
  aarch64-apple-darwin) GOARCH="arm64" ;;
  x86_64-apple-darwin)  GOARCH="amd64" ;;
  *) echo "unsupported triple: $TRIPLE" >&2; exit 1 ;;
esac

DEST="$DEST_DIR/rclone-$TRIPLE"
# Re-build when the patch changes: tag the staged binary with the version+patch hash.
STAMP="$DEST_DIR/.rclone-build-stamp"
WANT="$RCLONE_VERSION $(shasum -a 256 "$PATCH" | cut -d' ' -f1)"
if [ -x "$DEST" ] && [ "$(cat "$STAMP" 2>/dev/null || true)" = "$WANT" ]; then
  echo "✓ patched rclone already staged: $DEST ($("$DEST" version 2>/dev/null | head -1))"
  exit 0
fi

command -v go >/dev/null || { echo "Go is required to build the patched rclone" >&2; exit 1; }

mkdir -p "$DEST_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Cloning rclone $RCLONE_VERSION…"
git clone --depth 1 --branch "$RCLONE_VERSION" https://github.com/rclone/rclone.git "$TMP/rclone" 2>&1 | tail -1

echo "Applying xattr patch…"
git -C "$TMP/rclone" apply "$PATCH"

# cgofuse's CFLAGS hard-code -I/usr/local/include/fuse; put the vendored headers
# there so the build finds them without a macFUSE install.
echo "Staging FUSE headers…"
if mkdir -p /usr/local/include/fuse 2>/dev/null && [ -w /usr/local/include/fuse ]; then
  cp "$HEADERS"/*.h /usr/local/include/fuse/
else
  sudo mkdir -p /usr/local/include/fuse
  sudo cp "$HEADERS"/*.h /usr/local/include/fuse/
fi

echo "Building rclone (cmount, $GOARCH)…"
( cd "$TMP/rclone" && CGO_ENABLED=1 GOARCH="$GOARCH" go build -tags cmount -o "$DEST" . )
chmod +x "$DEST"
echo "$WANT" > "$STAMP"
echo "✓ Staged patched $DEST ($("$DEST" version | head -1))"
