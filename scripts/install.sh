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

# Check dependencies
check_dependencies() {
    # jj is required for workspace operations
    if ! command -v jj &> /dev/null; then
        warn "jj (Jujutsu) is recommended for bacchus workspace operations"
        warn "Install from: https://martinvonz.github.io/jj/latest/install/"
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

# Clean up legacy global skill/hooks from previous installs
cleanup_legacy() {
    # Remove old global skill directory
    local skill_dir="$HOME/.claude/skills/bacchus"
    if [ -d "$skill_dir" ]; then
        rm -rf "$skill_dir"
        info "Removed legacy global skill: $skill_dir"
    fi

    # Remove old plugin directory
    local plugin_dir="$HOME/.claude/plugins/bacchus"
    if [ -d "$plugin_dir" ]; then
        rm -rf "$plugin_dir"
        info "Removed legacy plugin: $plugin_dir"
    fi

    # Remove bacchus stop hook from global settings.json if present
    local settings_file="$HOME/.claude/settings.json"
    if [ -f "$settings_file" ] && command -v jq &> /dev/null; then
        local hook_cmd='bacchus session check 2>/dev/null || echo "{\"decision\":\"approve\"}"'
        if jq -e --arg cmd "$hook_cmd" \
            'any((.hooks.Stop // [])[]?.hooks[]?; .command == $cmd)' \
            "$settings_file" >/dev/null 2>&1; then
            local tmp_file="${settings_file}.tmp"
            jq --arg cmd "$hook_cmd" '
                .hooks = (.hooks // {}) |
                .hooks.Stop = (
                    (.hooks.Stop // [])
                    | map(
                        if (.hooks | type) == "array" then
                            .hooks |= map(select((.command // "") != $cmd))
                        else
                            .
                        end
                    )
                    | map(select(((.hooks | type) != "array") or ((.hooks | length) > 0)))
                ) |
                if (.hooks.Stop | length) == 0 then del(.hooks.Stop) else . end |
                if (.hooks | type) == "object" and (.hooks | length) == 0 then del(.hooks) else . end
            ' "$settings_file" > "$tmp_file" && mv "$tmp_file" "$settings_file"
            info "Removed legacy global stop hook from settings"
        fi
    fi
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

        # Clean up legacy global skill/hooks from previous installs
        cleanup_legacy

        info "Installation complete!"
        echo ""
        info "Next step: run 'bacchus init' in your project to set up:"
        echo ""
        echo "  - .bacchus/              Task database and workspaces"
        echo "  - .claude/settings.json  Project-level stop hook"
        echo "  - .claude/skills/        Bacchus skill for Claude Code"
        echo ""
        info "Usage:"
        echo "  cd your-project && bacchus init"
        echo ""
        "${INSTALL_DIR}/${BINARY_NAME}" --version 2>/dev/null || true
    else
        error "Installation failed"
    fi
}

main "$@"
