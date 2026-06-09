#!/usr/bin/env bash
# Usage: ./scripts/bump.sh [major|minor|patch]
# Bumps the version in tauri.conf.json, Cargo.toml, and package.json atomically.

set -euo pipefail

LEVEL="${1:-patch}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Read current version from tauri.conf.json
CURRENT=$(node -p "require('$ROOT/src-tauri/tauri.conf.json').version")

IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$LEVEL" in
  major) MAJOR=$((MAJOR+1)); MINOR=0; PATCH=0 ;;
  minor) MINOR=$((MINOR+1)); PATCH=0 ;;
  patch) PATCH=$((PATCH+1)) ;;
  *)     echo "Usage: bump.sh [major|minor|patch]"; exit 1 ;;
esac

NEXT="$MAJOR.$MINOR.$PATCH"
echo "Bumping $CURRENT → $NEXT"

# tauri.conf.json
node -e "
  const fs = require('fs');
  const p = '$ROOT/src-tauri/tauri.conf.json';
  const c = JSON.parse(fs.readFileSync(p,'utf8'));
  c.version = '$NEXT';
  fs.writeFileSync(p, JSON.stringify(c, null, 2) + '\n');
"

# package.json
node -e "
  const fs = require('fs');
  const p = '$ROOT/package.json';
  const c = JSON.parse(fs.readFileSync(p,'utf8'));
  c.version = '$NEXT';
  fs.writeFileSync(p, JSON.stringify(c, null, 2) + '\n');
"

# Cargo.toml  (sed is fine here — version line is always first occurrence)
sed -i '' "s/^version = \"$CURRENT\"/version = \"$NEXT\"/" "$ROOT/src-tauri/Cargo.toml"

echo "✓ Version bumped to $NEXT in tauri.conf.json, package.json, Cargo.toml"
echo "  Don't forget to update CHANGELOG.md and commit:"
echo "  git add -A && git commit -m \"chore: release v$NEXT\""
