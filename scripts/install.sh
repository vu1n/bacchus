#!/bin/bash
set -e

REPO="vu1n/bacchus"
INSTALL_DIR="${BACCHUS_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="bacchus"
SKILL_DIR="$HOME/.claude/skills/bacchus"
SETTINGS_FILE="$HOME/.claude/settings.json"

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

# Install Claude Code skill
# Usage: install_skill [tag]
#   tag: Git tag to download from (e.g., v0.4.0). Defaults to 'main' if not provided.
install_skill() {
    local tag="${1:-main}"
    info "Installing Claude Code skill (from $tag)..."

    # Remove old plugin directory if exists (migrating to skill)
    local old_plugin_dir="$HOME/.claude/plugins/bacchus"
    if [ -d "$old_plugin_dir" ]; then
        warn "Removing old plugin directory (migrating to skill)..."
        rm -rf "$old_plugin_dir"
    fi

    # Create skill directory
    mkdir -p "$SKILL_DIR"

    # Download skill files from repo at the specified tag
    local base_url="https://raw.githubusercontent.com/${REPO}/${tag}/skills/bacchus"

    curl -sLf -o "${SKILL_DIR}/SKILL.md" "${base_url}/SKILL.md" || warn "Could not download SKILL.md"
    curl -sLf -o "${SKILL_DIR}/archetypes.yaml" "${base_url}/archetypes.yaml" || warn "Could not download archetypes.yaml"

    info "Skill installed to: ${SKILL_DIR}"
    info "Archetypes available at: ${SKILL_DIR}/archetypes.yaml"
}

# Add stop hook to settings.json
install_hooks() {
    info "Configuring stop hooks..."

    # The hook command - fail-open design (approve if bacchus errors)
    local hook_cmd='bacchus session check 2>/dev/null || echo "{\"decision\":\"approve\"}"'

    # Check if settings file exists
    if [ -f "$SETTINGS_FILE" ]; then
        # Check if jq is available for JSON manipulation
        if command -v jq &> /dev/null; then
            # Check if hooks.Stop already exists with our command
            if jq -e '.hooks.Stop[0].hooks[0].command' "$SETTINGS_FILE" 2>/dev/null | grep -q "bacchus session check"; then
                info "Stop hook already configured"
                return
            fi

            # Add or merge hooks
            local tmp_file="${SETTINGS_FILE}.tmp"
            jq '.hooks.Stop = [{"hooks": [{"type": "command", "command": "'"$hook_cmd"'"}]}]' "$SETTINGS_FILE" > "$tmp_file" && mv "$tmp_file" "$SETTINGS_FILE"
            info "Stop hook added to existing settings"
        else
            warn "jq not found - cannot automatically add hooks to settings.json"
            warn "Please add the following to $SETTINGS_FILE manually:"
            echo ""
            echo '  "hooks": {'
            echo '    "Stop": [{"hooks": [{"type": "command", "command": "'"$hook_cmd"'"}]}]'
            echo '  }'
            echo ""
        fi
    else
        # Create new settings file with hooks
        mkdir -p "$(dirname "$SETTINGS_FILE")"
        cat > "$SETTINGS_FILE" << EOF
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$hook_cmd"
          }
        ]
      }
    ]
  }
}
EOF
        info "Created settings file with stop hook"
    fi
}

# Get latest release tag from GitHub
get_latest_tag() {
    curl -sL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/'
}

# Main installation
main() {
    # Check dependencies first
    check_dependencies

    local os arch release_tag
    os=$(detect_os)
    arch=$(detect_arch)

    info "Detected: ${os}-${arch}"

    # Get latest release tag (used for both binary and plugin)
    release_tag=$(get_latest_tag)
    if [ -z "$release_tag" ]; then
        warn "Could not determine latest release, using main branch"
        release_tag="main"
    else
        info "Latest release: $release_tag"
    fi

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

        # Install Claude Code skill and hooks
        install_skill "$release_tag"
        install_hooks

        info "Installation complete!"
        echo ""
        info "Bacchus is now ready to use:"
        echo ""
        echo "  1. The skill is available at: ~/.claude/skills/bacchus/"
        echo "  2. Stop hooks are configured in: ~/.claude/settings.json"
        echo ""
        info "Usage:"
        echo "  - Ask Claude to 'use bacchus to parallelize this work'"
        echo "  - In a repo, run 'bacchus init --epic-id MY-EPIC' for one-shot bootstrap"
        echo "  - Or run 'bacchus task init' to create a task file only"
        echo ""
        "${INSTALL_DIR}/${BINARY_NAME}" --version 2>/dev/null || true
    else
        error "Installation failed"
    fi
}

main "$@"
