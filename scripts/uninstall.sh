#!/bin/bash
set -e

INSTALL_DIR="${BACCHUS_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="bacchus"

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

    echo ""
    read -p "Also remove .bacchus data directories? [y/N] " -n 1 -r
    echo ""

    if [[ $REPLY =~ ^[Yy]$ ]]; then
        cleanup_data
    fi

    echo ""
    info "Uninstall complete!"
}

main "$@"
