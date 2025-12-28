#!/bin/bash
set -e

INSTALL_DIR="${BACCHUS_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="bacchus"
SKILL_DIR="$HOME/.claude/skills/bacchus"
PLUGIN_DIR="$HOME/.claude/plugins/bacchus"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# Remove binary
remove_binary() {
    local binary_path="${INSTALL_DIR}/${BINARY_NAME}"

    if [ -f "$binary_path" ]; then
        rm "$binary_path"
        info "Removed: $binary_path"
    else
        warn "Binary not found: $binary_path"
    fi
}

# Remove Claude Code plugin and skills
remove_plugin() {
    # Remove plugin
    if [ -d "$PLUGIN_DIR" ]; then
        rm -rf "$PLUGIN_DIR"
        info "Removed plugin: $PLUGIN_DIR"
    fi

    # Remove old skill directory (legacy)
    if [ -d "$SKILL_DIR" ]; then
        rm -rf "$SKILL_DIR"
        info "Removed skills: $SKILL_DIR"
    fi
}

# Remove session files from .bacchus directories
cleanup_sessions() {
    local session_files
    session_files=$(find "$HOME" -maxdepth 5 -path "*/.bacchus/session.json" 2>/dev/null || true)

    if [ -n "$session_files" ]; then
        echo "$session_files" | while read -r file; do
            rm -f "$file"
            info "Removed session: $file"
        done
    fi
}

# Find and optionally remove .bacchus directories
cleanup_data() {
    local dirs
    dirs=$(find "$HOME" -maxdepth 4 -type d -name ".bacchus" 2>/dev/null || true)

    if [ -z "$dirs" ]; then
        info "No .bacchus directories found"
        return
    fi

    echo ""
    warn "Found .bacchus directories:"
    echo "$dirs" | while read -r dir; do
        echo "  $dir"
    done
    echo ""

    read -p "Remove these directories? [y/N] " -n 1 -r
    echo ""

    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo "$dirs" | while read -r dir; do
            rm -rf "$dir"
            info "Removed: $dir"
        done
    else
        info "Skipped directory cleanup"
    fi
}

main() {
    info "Uninstalling bacchus..."
    echo ""

    remove_binary
    remove_plugin
    cleanup_sessions

    echo ""
    read -p "Also remove .bacchus data directories (worktrees, database)? [y/N] " -n 1 -r
    echo ""

    if [[ $REPLY =~ ^[Yy]$ ]]; then
        cleanup_data
    fi

    echo ""
    info "Uninstall complete!"
    info "Restart Claude Code to complete plugin removal"
}

main "$@"
