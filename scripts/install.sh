#!/bin/bash
set -e

REPO="vu1n/bacchus"
INSTALL_DIR="${BACCHUS_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="bacchus"
SKILL_DIR="$HOME/.claude/skills/bacchus"
PLUGIN_DIR="$HOME/.claude/plugins/bacchus"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# Check dependencies
check_dependencies() {
    local missing=0

    # bd (beads CLI) is required for bead management
    if ! command -v bd &> /dev/null; then
        warn "bd (beads CLI) not found. Bacchus requires beads for task tracking."
        warn "Install beads: https://github.com/vu1n/beads"
        missing=1
    fi

    # git is required for worktree operations
    if ! command -v git &> /dev/null; then
        error "git is required for bacchus worktree operations"
    fi

    if [ $missing -eq 1 ]; then
        warn "Some dependencies missing. Bacchus may not work correctly."
        echo ""
    fi
}

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
    local temp_binary="${INSTALL_DIR}/${BINARY_NAME}.tmp"

    info "Downloading from: $download_url"

    # Download to temp file first
    if curl -sLf -o "$temp_binary" "$download_url"; then
        chmod +x "$temp_binary"

        # Atomic replace
        if mv "$temp_binary" "${INSTALL_DIR}/${BINARY_NAME}"; then
            info "Binary installed successfully"
            return 0
        else
            error "Failed to move binary to ${INSTALL_DIR}/${BINARY_NAME}"
        fi
    else
        warn "Binary not available for ${os}-${arch}"
        rm -f "$temp_binary"  # Clean up partial download
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
    local temp_binary="${INSTALL_DIR}/${BINARY_NAME}.tmp"
    cp "target/release/${BINARY_NAME}" "$temp_binary"
    chmod +x "$temp_binary"

    # Atomic replace
    if mv "$temp_binary" "${INSTALL_DIR}/${BINARY_NAME}"; then
        info "Binary installed successfully"
    else
        error "Failed to move binary to ${INSTALL_DIR}/${BINARY_NAME}"
    fi
}

# Install Claude Code plugin (includes hooks, commands, skills)
install_plugin() {
    info "Installing Claude Code plugin..."

    # Remove old skill directory if exists (migrating to plugin)
    if [ -d "$SKILL_DIR" ]; then
        warn "Removing old skill directory (migrating to plugin)..."
        rm -rf "$SKILL_DIR"
    fi

    # Create plugin directory
    mkdir -p "$PLUGIN_DIR"

    # Download plugin files from repo
    local base_url="https://raw.githubusercontent.com/${REPO}/main/plugin"

    # Plugin config
    mkdir -p "${PLUGIN_DIR}/.claude-plugin"
    curl -sLf -o "${PLUGIN_DIR}/.claude-plugin/config.json" "${base_url}/.claude-plugin/config.json" || warn "Could not download config.json"

    # Hooks
    mkdir -p "${PLUGIN_DIR}/hooks"
    curl -sLf -o "${PLUGIN_DIR}/hooks/hooks.json" "${base_url}/hooks/hooks.json" || warn "Could not download hooks.json"
    curl -sLf -o "${PLUGIN_DIR}/hooks/stop-router.sh" "${base_url}/hooks/stop-router.sh" || warn "Could not download stop-router.sh"
    chmod +x "${PLUGIN_DIR}/hooks/stop-router.sh" 2>/dev/null || true

    # Scripts
    mkdir -p "${PLUGIN_DIR}/scripts"
    curl -sLf -o "${PLUGIN_DIR}/scripts/session.sh" "${base_url}/scripts/session.sh" || warn "Could not download session.sh"
    chmod +x "${PLUGIN_DIR}/scripts/session.sh" 2>/dev/null || true

    # Commands
    mkdir -p "${PLUGIN_DIR}/commands"
    curl -sLf -o "${PLUGIN_DIR}/commands/agent.md" "${base_url}/commands/agent.md" || warn "Could not download agent.md"
    curl -sLf -o "${PLUGIN_DIR}/commands/orchestrate.md" "${base_url}/commands/orchestrate.md" || warn "Could not download orchestrate.md"
    curl -sLf -o "${PLUGIN_DIR}/commands/cancel.md" "${base_url}/commands/cancel.md" || warn "Could not download cancel.md"

    # Skills
    mkdir -p "${PLUGIN_DIR}/skills"
    curl -sLf -o "${PLUGIN_DIR}/skills/planner.md" "${base_url}/skills/planner.md" || warn "Could not download planner.md"
    curl -sLf -o "${PLUGIN_DIR}/skills/context.md" "${base_url}/skills/context.md" || warn "Could not download context.md"

    # README
    curl -sLf -o "${PLUGIN_DIR}/README.md" "${base_url}/README.md" || warn "Could not download README.md"

    info "Plugin installed to: ${PLUGIN_DIR}"
    info "Available commands: /bacchus-agent, /bacchus-orchestrate, /bacchus-cancel"
}

# Main installation
main() {
    # Check dependencies first
    check_dependencies

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
        info "Binary installed to: ${INSTALL_DIR}/${BINARY_NAME}"

        # Check if in PATH
        if ! echo "$PATH" | grep -q "$INSTALL_DIR"; then
            warn "Add ${INSTALL_DIR} to your PATH:"
            echo ""
            echo "  export PATH=\"\$PATH:${INSTALL_DIR}\""
            echo ""
            echo "Add this to your ~/.bashrc or ~/.zshrc"
        fi

        # Install Claude Code plugin
        install_plugin

        info "Installation complete!"
        info "Restart Claude Code to activate the plugin"
        "${INSTALL_DIR}/${BINARY_NAME}" --version 2>/dev/null || true
    else
        error "Installation failed"
    fi
}

main "$@"
