# Bacchus Plugin for Claude Code

Multi-agent coordination with persistent stop hooks. Agents keep working until beads are closed. Orchestrator spawns agents for ready work.

## Installation

```bash
# Via install script (recommended)
curl -fsSL https://raw.githubusercontent.com/vu1n/bacchus/main/scripts/install.sh | bash

# Or manually symlink
ln -s /path/to/bacchus/plugin ~/.claude/plugins/bacchus
```

Restart Claude Code after installation.

## Prerequisites

- [bacchus CLI](https://github.com/vu1n/bacchus) v0.4.0+ installed and in PATH
- [beads CLI](https://github.com/vu1n/beads) installed (`bd` command)

Note: The stop hook gracefully degrades (approves exit) if dependencies are missing or error.

## How It Works

The plugin uses **file-based session state** stored in `.bacchus/session.json`:

```json
{
  "mode": "agent",
  "bead_id": "BACH-xxx",
  "started_at": "2025-01-01T00:00:00Z"
}
```

The stop hook reads this file to decide whether to block exit:

```
┌─────────────────────────────────────────────────────┐
│                   ORCHESTRATOR                       │
│  session.json: {mode: "orchestrator"}               │
│  Stop Hook: Check bd ready → spawn if work exists   │
├─────────────────────────────────────────────────────┤
│  ┌─────────┐  ┌─────────┐  ┌─────────┐             │
│  │ Agent 1 │  │ Agent 2 │  │ Agent 3 │             │
│  │ BACH-X  │  │ BACH-Y  │  │ BACH-Z  │             │
│  └────┬────┘  └────┬────┘  └────┬────┘             │
│       │            │            │                   │
│  session.json: {mode: "agent", bead_id: "..."}     │
│  Stop Hook: Check bd show → block if not closed    │
└─────────────────────────────────────────────────────┘
```

## Commands

### `/bacchus-agent <bead_id>`

Start a persistent agent on a single bead.

```
/bacchus-agent BACH-abc123
```

Creates a session file and blocks exit until the bead is closed. Session auto-clears on completion.

### `/bacchus-orchestrate [--max_concurrent N]`

Start orchestrator that manages multiple agents.

```
/bacchus-orchestrate --max_concurrent 5
```

Spawns agents for ready beads and monitors progress. Session auto-clears when all work is done.

### `/bacchus-cancel [--cleanup]`

Cancel active session and allow normal exit.

```
/bacchus-cancel --cleanup
```

## Session Management

Use the bacchus CLI for session management:

```bash
# Start agent session
bacchus session start agent --bead-id BACH-xxx

# Start orchestrator session
bacchus session start orchestrator --max-concurrent 5

# Stop session
bacchus session stop

# Check status
bacchus session status

# Check for stop hook (returns JSON decision)
bacchus session check
```

Session file location: `.bacchus/session.json` in workspace root.

## Stop Hook Logic

### Agent Mode

```
Read .bacchus/session.json
If mode != "agent" → APPROVE
If bead_id missing → APPROVE

bd show $bead_id --json
  → status == "closed" → APPROVE (auto-clear session)
  → status != "closed" → BLOCK
```

### Orchestrator Mode

```
Read .bacchus/session.json
If mode != "orchestrator" → APPROVE

bd ready --json          → ready_count
bd status --json         → open/in_progress/blocked counts
bacchus list             → active_count

if ready_count > 0 AND active_count < max_concurrent:
  → BLOCK (spawn more agents)
elif in_progress_count > 0 OR active_count > 0:
  → BLOCK (wait for completion)
elif open_count > 0 AND ready_count == 0:
  → APPROVE (all blocked, auto-clear session)
else:
  → APPROVE (all complete, auto-clear session)
```

## Skills

### `/bacchus-planner`

Break down complex requests into beads with dependencies.

### `/bacchus-context`

Generate context summary for current session.

## Troubleshooting

### Agent won't exit

Check if bead is closed:
```bash
bd show $bead_id
bd close $bead_id  # If ready to close
```

Or force exit:
```bash
bacchus session stop
```

### Check session state

```bash
bacchus session status
# Or directly:
cat .bacchus/session.json
```

### Clear stale session

```bash
bacchus session stop
# Or manually:
rm .bacchus/session.json
```

## Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/vu1n/bacchus/main/scripts/uninstall.sh | bash
```

This removes:
- Binary from `~/.local/bin/`
- Plugin from `~/.claude/plugins/bacchus/`
- Session files from `.bacchus/` directories

## Development

Test the hook locally:
```bash
# No session → approves
bacchus session check

# Create test session
bacchus session start agent --bead-id BACH-xxx

# Check with session → blocks (if bead not closed)
bacchus session check

# Test via shell hook
echo '{}' | CLAUDE_PROJECT_DIR=$(pwd) ./hooks/stop-router.sh

# Cleanup
bacchus session stop
```

## Related

- [bacchus CLI](https://github.com/vu1n/bacchus) - Coordination primitives
- [beads](https://github.com/anthropics/beads) - Issue tracking
- [ralph-wiggum](https://github.com/anthropics/claude-plugins-official/tree/main/plugins/ralph-wiggum) - Original stop hook inspiration
