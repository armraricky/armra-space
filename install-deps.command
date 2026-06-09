#!/usr/bin/env bash
# ARMRA Space — Dependency Installer
# Double-click this once before first launch.
# Works on macOS 12+. Requires internet access.

set -euo pipefail

BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
RESET='\033[0m'

header() { echo -e "\n${BOLD}$1${RESET}"; }
ok()     { echo -e "  ${GREEN}✓${RESET} $1"; }
warn()   { echo -e "  ${YELLOW}⚠${RESET}  $1"; }
die()    { echo -e "  ${RED}✗${RESET} $1"; exit 1; }

clear
echo -e "${BOLD}ARMRA Space — Dependency Installer${RESET}"
echo -e "${DIM}This installs rclone and macFUSE, which are required for S3 mounting.${RESET}"
echo ""

# ── 1. Homebrew ──────────────────────────────────────────────────────────────
header "Checking Homebrew…"
if command -v brew &>/dev/null; then
    ok "Homebrew already installed ($(brew --version | head -1))"
else
    warn "Homebrew not found — installing…"
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    # Add brew to PATH for Apple Silicon
    if [[ -f /opt/homebrew/bin/brew ]]; then
        eval "$(/opt/homebrew/bin/brew shellenv)"
    fi
    ok "Homebrew installed"
fi

# ── 2. rclone ────────────────────────────────────────────────────────────────
header "Checking rclone…"
if command -v rclone &>/dev/null; then
    ok "rclone already installed ($(rclone --version | head -1))"
else
    warn "rclone not found — installing…"
    brew install rclone
    ok "rclone installed"
fi

# ── 3. macFUSE ───────────────────────────────────────────────────────────────
header "Checking macFUSE…"
if [[ -d /Library/Filesystems/macfuse.fs ]]; then
    ok "macFUSE already installed"
else
    warn "macFUSE not found — installing…"
    echo -e "  ${DIM}You may be prompted for your password and to approve a system extension.${RESET}"
    brew install --cask macfuse || {
        echo ""
        warn "If brew failed, download macFUSE manually from https://osxfuse.github.io"
        warn "Then re-run this script."
        exit 1
    }
    ok "macFUSE installed"
fi

# ── 4. Done ──────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${GREEN}All dependencies installed.${RESET}"
echo ""

# macFUSE requires approving a kernel extension on first install
if ! kextstat 2>/dev/null | grep -q "macfuse\|osxfuse"; then
    echo -e "${YELLOW}Action required:${RESET}"
    echo "  1. Open System Settings → Privacy & Security"
    echo "  2. Scroll down and allow the macFUSE system extension"
    echo "  3. Restart your Mac"
    echo "  4. Then drag ${BOLD}ARMRA Space.app${RESET} to your Applications folder"
    echo ""
    read -n1 -r -p "Press any key to open System Settings now… " _
    open "x-apple.systempreferences:com.apple.preference.security"
else
    echo "  Drag ${BOLD}ARMRA Space.app${RESET} to your Applications folder and launch it."
    echo ""
fi
