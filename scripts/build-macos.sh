#!/usr/bin/env bash
# Build ARMRA Space.dmg for macOS distribution.
# Outputs to: src-tauri/target/release/bundle/dmg/

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
echo "Building ARMRA Space v$VERSION for macOS…"

# Ensure install-deps.command is executable (it ships inside the DMG)
chmod +x "$ROOT/install-deps.command"

npm run tauri:build -- --target aarch64-apple-darwin

DMG_PATH=$(find "$ROOT/src-tauri/target/release/bundle/dmg" -name "*.dmg" | head -1)
echo ""
echo "✓ Build complete"
echo "  $DMG_PATH"
echo ""
echo "To notarize (requires Apple Developer account):"
echo "  xcrun notarytool submit \"$DMG_PATH\" --apple-id YOU@EMAIL --team-id TEAMID --wait"
