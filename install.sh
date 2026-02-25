#!/bin/sh
# My-Agent installer
# Usage: curl -fsSL https://raw.githubusercontent.com/Rapheal7/My-Agent/main/install.sh | sh
set -e

REPO="Rapheal7/My-Agent"
BINARY_NAME="my-agent"

# Detect OS
OS="$(uname -s)"
case "$OS" in
    Linux)  OS_TARGET="unknown-linux-gnu" ;;
    Darwin) OS_TARGET="apple-darwin" ;;
    *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64|amd64)   ARCH_TARGET="x86_64" ;;
    aarch64|arm64)   ARCH_TARGET="aarch64" ;;
    *)               echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

TARGET="${ARCH_TARGET}-${OS_TARGET}"
ASSET_NAME="${BINARY_NAME}-${TARGET}.tar.gz"

echo "Detecting system: ${OS} ${ARCH} -> ${TARGET}"

# Fetch latest release tag
echo "Fetching latest release..."
RELEASE_URL="https://api.github.com/repos/${REPO}/releases/latest"
TAG=$(curl -fsSL "$RELEASE_URL" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$TAG" ]; then
    echo "Error: Could not determine latest release tag."
    exit 1
fi

echo "Latest release: ${TAG}"

# Download asset
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET_NAME}"
echo "Downloading ${ASSET_NAME}..."

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

if ! curl -fsSL -o "${TMPDIR}/${ASSET_NAME}" "$DOWNLOAD_URL" 2>/dev/null; then
    # Fallback: on macOS x86_64, try the ARM64 build (runs via Rosetta 2)
    if [ "$OS" = "Darwin" ] && [ "$ARCH_TARGET" = "x86_64" ]; then
        echo "No x86_64 macOS build available, trying ARM64 (Rosetta 2)..."
        ARCH_TARGET="aarch64"
        TARGET="${ARCH_TARGET}-${OS_TARGET}"
        ASSET_NAME="${BINARY_NAME}-${TARGET}.tar.gz"
        DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET_NAME}"
        curl -fsSL -o "${TMPDIR}/${ASSET_NAME}" "$DOWNLOAD_URL"
    else
        echo "Error: Could not download ${ASSET_NAME}"
        echo "URL: ${DOWNLOAD_URL}"
        exit 1
    fi
fi

# Extract
echo "Extracting..."
tar xzf "${TMPDIR}/${ASSET_NAME}" -C "$TMPDIR"

# Determine install directory
if [ -d "$HOME/.cargo/bin" ]; then
    INSTALL_DIR="$HOME/.cargo/bin"
elif [ -d "$HOME/.local/bin" ]; then
    INSTALL_DIR="$HOME/.local/bin"
else
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
    echo "Created ${INSTALL_DIR} — ensure it is in your PATH:"
    echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
fi

# Install
cp "${TMPDIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

echo ""
echo "Installed ${BINARY_NAME} ${TAG} to ${INSTALL_DIR}/${BINARY_NAME}"

# Verify
if command -v "$BINARY_NAME" >/dev/null 2>&1; then
    echo "Version: $($BINARY_NAME --version)"
else
    echo ""
    echo "Note: ${BINARY_NAME} is not in your PATH yet."
    echo "Add this to your shell profile:"
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
fi

echo ""
echo "Get started:"
echo "  ${BINARY_NAME} config --set-api-key YOUR_OPENROUTER_KEY"
echo "  ${BINARY_NAME} config --set-password"
echo "  ${BINARY_NAME}"
