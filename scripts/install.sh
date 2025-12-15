#!/bin/bash
set -e

REPO="vu1n/bacchus"
INSTALL_DIR="${BACCHUS_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="bacchus"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# Detect OS
detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "darwin" ;;
        *)       error "Unsupported OS: $(uname -s)" ;;
    esac
}

# Detect architecture
detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)  echo "x86_64" ;;
        arm64|aarch64) echo "aarch64" ;;
        *)             error "Unsupported architecture: $(uname -m)" ;;
    esac
}

# Try to download pre-built binary
try_download_binary() {
    local os="$1"
    local arch="$2"

    info "Checking for pre-built binary..."

    # Get latest release tag
    local latest_tag
    latest_tag=$(curl -sL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')

    if [ -z "$latest_tag" ]; then
        warn "No releases found"
        return 1
    fi

    info "Latest release: $latest_tag"

    local binary_name="bacchus-${os}-${arch}"
    local download_url="https://github.com/${REPO}/releases/download/${latest_tag}/${binary_name}"

    info "Downloading from: $download_url"

    if curl -sLf -o "${INSTALL_DIR}/${BINARY_NAME}" "$download_url"; then
        chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
        return 0
    else
        warn "Binary not available for ${os}-${arch}"
        return 1
    fi
}

# Build from source
build_from_source() {
    info "Building from source..."

    # Check for cargo
    if ! command -v cargo &> /dev/null; then
        warn "Cargo not found. Installing Rust..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source "$HOME/.cargo/env"
    fi

    # Create temp directory
    local tmp_dir
    tmp_dir=$(mktemp -d)
    trap "rm -rf $tmp_dir" EXIT

    info "Cloning repository..."
    git clone --depth 1 "https://github.com/${REPO}.git" "$tmp_dir"

    info "Building release binary..."
    cd "$tmp_dir"
    cargo build --release

    info "Installing binary..."
    cp "target/release/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
}

# Main installation
main() {
    local os arch
    os=$(detect_os)
    arch=$(detect_arch)

    info "Detected: ${os}-${arch}"

    # Create install directory
    mkdir -p "$INSTALL_DIR"

    # Try binary download first, fall back to source
    if ! try_download_binary "$os" "$arch"; then
        build_from_source
    fi

    # Verify installation
    if [ -x "${INSTALL_DIR}/${BINARY_NAME}" ]; then
        info "Installed to: ${INSTALL_DIR}/${BINARY_NAME}"

        # Check if in PATH
        if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
            warn "Add ${INSTALL_DIR} to your PATH:"
            echo ""
            echo "  export PATH=\"\$PATH:${INSTALL_DIR}\""
            echo ""
            echo "Add this to your ~/.bashrc or ~/.zshrc"
        fi

        info "Installation complete!"
        "${INSTALL_DIR}/${BINARY_NAME}" --version 2>/dev/null || true
    else
        error "Installation failed"
    fi
}

main "$@"
