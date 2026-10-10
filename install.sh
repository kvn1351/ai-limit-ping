#!/usr/bin/env sh
set -e

# ai-limit-ping (alp) 1-line installer for macOS and Linux
# Repository: https://github.com/kvn1351/ai-limit-ping

REPO="kvn1351/ai-limit-ping"
INSTALL_DIR="${ALP_INSTALL_DIR:-$HOME/.local/bin}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

printf "${BOLD}${CYAN}→ Installing ai-limit-ping (alp)...${NC}\n"

# 1. Detect OS
OS="$(uname -s)"
case "$OS" in
    Darwin)
        OS_TARGET="apple-darwin"
        ;;
    Linux)
        OS_TARGET="unknown-linux-gnu"
        ;;
    *)
        printf "${RED}Unsupported operating system: %s${NC}\n" "$OS"
        printf "For Windows, install via PowerShell: irm https://raw.githubusercontent.com/%s/main/install.ps1 | iex\n" "$REPO"
        exit 1
        ;;
esac

# 2. Detect Architecture
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64|amd64)
        ARCH_TARGET="x86_64"
        ;;
    arm64|aarch64)
        ARCH_TARGET="aarch64"
        ;;
    *)
        printf "${RED}Unsupported CPU architecture: %s${NC}\n" "$ARCH"
        exit 1
        ;;
esac

TARGET="${ARCH_TARGET}-${OS_TARGET}"
DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/alp-${TARGET}.tar.gz"

printf "  Detected platform: ${BOLD}%s${NC}\n" "$TARGET"
printf "  Destination:       ${BOLD}%s/alp${NC}\n" "$INSTALL_DIR"

# 3. Create install directory
mkdir -p "$INSTALL_DIR"

# 4. Download and extract
TMP_DIR="$(mktemp -d)"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

printf "  Downloading latest release from GitHub...\n"
if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/alp.tar.gz"
elif command -v wget >/dev/null 2>&1; then
    wget -q "$DOWNLOAD_URL" -O "$TMP_DIR/alp.tar.gz"
else
    printf "${RED}Error: Neither curl nor wget is installed.${NC}\n"
    exit 1
fi

tar -xzf "$TMP_DIR/alp.tar.gz" -C "$TMP_DIR"

# Install binary
mv "$TMP_DIR/alp" "$INSTALL_DIR/alp"
chmod +x "$INSTALL_DIR/alp"

# On macOS, clear quarantine and ad-hoc sign to avoid gatekeeper/SIGKILL
if [ "$OS" = "Darwin" ]; then
    xattr -d com.apple.quarantine "$INSTALL_DIR/alp" 2>/dev/null || true
    codesign -s - -f "$INSTALL_DIR/alp" 2>/dev/null || true
fi

printf "\n${GREEN}${BOLD}✓ alp successfully installed!${NC}\n\n"

# 5. Check if INSTALL_DIR is in PATH
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        printf "${BOLD}Note:${NC} %s is not in your PATH.\n" "$INSTALL_DIR"
        printf "Add it by running:\n"
        printf "  export PATH=\"%s:\$PATH\"\n\n" "$INSTALL_DIR"
        printf "Or add that line to your ~/.zshrc or ~/.bashrc\n\n"
        ;;
esac

printf "Get started:\n"
printf "  alp status          # View 5-hour limit windows\n"
printf "  alp daemon install  # Enable unattended background service\n"
