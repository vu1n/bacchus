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

# Remove legacy global skill/plugin/hooks from previous installs
remove_legacy_global() {
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
            info "Removed legacy global stop hook"
        fi
    fi
}

# Remove project-level bacchus hook from .claude/settings.json
remove_project_hook() {
    local settings_file="$1"

    if [ ! -f "$settings_file" ] || ! command -v jq &> /dev/null; then
        return
    fi

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
        info "Removed project hook: $settings_file"
    fi
}

# Find and clean up .bacchus directories
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
            # Also clean project-level Claude integration
            local project_root
            project_root=$(dirname "$dir")
            remove_project_hook "$project_root/.claude/settings.json"

            local project_skill="$project_root/.claude/skills/bacchus"
            if [ -d "$project_skill" ]; then
                rm -rf "$project_skill"
                info "Removed project skill: $project_skill"
            fi

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
    remove_legacy_global

    echo ""
    read -p "Also remove .bacchus data directories and project-level hooks/skills? [y/N] " -n 1 -r
    echo ""

    if [[ $REPLY =~ ^[Yy]$ ]]; then
        cleanup_data
    fi

    echo ""
    info "Uninstall complete!"
}

main "$@"
