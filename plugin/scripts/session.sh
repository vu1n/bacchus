#!/bin/bash
# Bacchus session management helper
# Usage:
#   session.sh start agent <bead_id>
#   session.sh start orchestrator [max_concurrent]
#   session.sh stop
#   session.sh status

set -euo pipefail

# Find workspace root
find_workspace_root() {
    local dir="${CLAUDE_PROJECT_DIR:-$(pwd)}"
    while [[ "$dir" != "/" ]]; do
        if [[ -d "$dir/.bacchus" ]] || [[ -d "$dir/.beads" ]]; then
            echo "$dir"
            return 0
        fi
        dir=$(dirname "$dir")
    done
    # Default to current dir and create .bacchus
    echo "$(pwd)"
}

WORKSPACE_ROOT=$(find_workspace_root)
BACCHUS_DIR="${WORKSPACE_ROOT}/.bacchus"
SESSION_FILE="${BACCHUS_DIR}/session.json"

# Ensure .bacchus directory exists
mkdir -p "$BACCHUS_DIR"

case "${1:-}" in
    start)
        mode="${2:-}"
        case "$mode" in
            agent)
                bead_id="${3:-}"
                if [[ -z "$bead_id" ]]; then
                    echo "Error: bead_id required for agent mode" >&2
                    exit 1
                fi
                cat > "$SESSION_FILE" <<EOF
{
  "mode": "agent",
  "bead_id": "$bead_id",
  "started_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
                echo "Started agent session for $bead_id"
                ;;
            orchestrator)
                max_concurrent="${3:-3}"
                cat > "$SESSION_FILE" <<EOF
{
  "mode": "orchestrator",
  "max_concurrent": $max_concurrent,
  "started_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
                echo "Started orchestrator session (max_concurrent: $max_concurrent)"
                ;;
            *)
                echo "Error: mode must be 'agent' or 'orchestrator'" >&2
                exit 1
                ;;
        esac
        ;;

    stop)
        if [[ -f "$SESSION_FILE" ]]; then
            rm -f "$SESSION_FILE"
            echo "Session stopped"
        else
            echo "No active session"
        fi
        ;;

    status)
        if [[ -f "$SESSION_FILE" ]]; then
            echo "Session file: $SESSION_FILE"
            cat "$SESSION_FILE"
        else
            echo "No active session"
        fi
        ;;

    *)
        echo "Usage: session.sh {start agent <bead_id>|start orchestrator [max]|stop|status}"
        exit 1
        ;;
esac
