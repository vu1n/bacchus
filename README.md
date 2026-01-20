# Bacchus

Worktree-based coordination CLI for multi-agent work on codebases.

Bacchus helps AI agents coordinate when working on the same codebase by:
- **Worktree isolation** - each agent works in its own git worktree
- **Task management** - SQLite-based task tracking with dependencies and footprints
- **Session management** - stop hooks keep agents working until tasks complete
- **Stale detection** - finds and cleans up abandoned work

## Installation

```bash
curl -fsSL https://raw.githubusercontent.com/vu1n/bacchus/main/scripts/install.sh | bash
```

This installs:
- `bacchus` binary to `~/.local/bin/`
- Claude Code plugin to `~/.claude/plugins/bacchus/`

### Prerequisites

- git

### From Source

```bash
git clone https://github.com/vu1n/bacchus.git
cd bacchus
cargo build --release
cp target/release/bacchus ~/.local/bin/
```

### Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/vu1n/bacchus/main/scripts/uninstall.sh | bash
```

## Quick Start

```bash
# Initialize tasks
bacchus task init
# Edit .bacchus/tasks.yaml to define tasks
bacchus task import --epic-id MY-EPIC

# Get next ready task (creates worktree, claims it)
bacchus next agent-1

# Or claim a specific task
bacchus claim TASK-42 agent-1

# Work in the isolated worktree (use -C flag, don't cd)
git -C .bacchus/worktrees/TASK-42 status
# ... make changes, commit ...

# Release when done (merges to main, cleans up)
bacchus release TASK-42 --status done
```

## Commands

### Task Management

| Command | Description |
|---------|-------------|
| `task init` | Create tasks.yaml template |
| `task list [--status X] [--ready]` | List tasks |
| `task show <task_id>` | Show task details |
| `task import [--epic-id X]` | Import tasks from YAML to SQLite |
| `task validate` | Validate task definitions |

### Coordination

| Command | Description |
|---------|-------------|
| `next <agent_id>` | Get next ready task, create worktree, claim it |
| `claim <task_id> <agent_id> [--force]` | Claim specific task (must be ready unless --force) |
| `release <task_id> --status done\|blocked\|failed` | Finish work |
| `stale [--minutes N] [--cleanup]` | Find/cleanup abandoned claims |
| `list` | List all active claims |
| `resolve <task_id>` | Complete merge after resolving conflicts |
| `abort <task_id>` | Abort merge, keep working |

### Review & Eval

| Command | Description |
|---------|-------------|
| `review <task_id> [--build-cmd X] [--test-cmd Y]` | Review task before release |
| `eval [--epic X] [--days N]` | Show completion metrics |

### Session Management

| Command | Description |
|---------|-------------|
| `session start agent --task-id <id>` | Start agent session (enables stop hook) |
| `session start orchestrator [--max-concurrent N]` | Start orchestrator session |
| `session stop` | Clear session, allow exit |
| `session status` | Show current session state |
| `session check` | Check if exit should be blocked (for hooks) |

### Symbols

| Command | Description |
|---------|-------------|
| `index <path>` | Index files for symbol search |
| `symbols [--pattern X] [--kind Y]` | Search for symbols |

### Info

| Command | Description |
|---------|-------------|
| `status` | Show claims, orphaned worktrees, broken claims |
| `context [--task-id X]` | Generate markdown context for agent |
| `workflow` | Print protocol documentation |

## Claude Code Plugin

The plugin provides stop hooks that keep agents working until tasks complete:

### Agent Mode

```
/bacchus-agent TASK-42
```

Starts an agent session. The stop hook blocks exit until the task is closed.

### Orchestrator Mode

```
/bacchus-orchestrate --max_concurrent 3
```

Spawns agents for ready tasks and monitors progress. Blocks exit while work remains.

### Cancel Session

```
/bacchus-cancel
```

Clears session and allows normal exit.

## Workflow

```
claim/next → work in worktree → release
```

### 1. Get Work

```bash
# Option A: Next ready task
bacchus next agent-1

# Option B: Specific task
bacchus claim TASK-42 agent-1
```

Output:
```json
{
  "success": true,
  "task_id": "TASK-42",
  "title": "Implement auth",
  "worktree_path": ".bacchus/worktrees/TASK-42",
  "branch": "bacchus/TASK-42"
}
```

### 2. Do Work

Work in the worktree. All changes are isolated on branch `bacchus/{task_id}`.

> **Warning**: Never `cd` into a worktree. Use `git -C` instead - worktrees are ephemeral and get deleted on release.

```bash
git -C .bacchus/worktrees/TASK-42 status
git -C .bacchus/worktrees/TASK-42 add .
git -C .bacchus/worktrees/TASK-42 commit -m "Implement auth"
```

### 3. Release

```bash
# Success - merge to main, cleanup worktree, close task
bacchus release TASK-42 --status done

# Blocked - keep worktree, mark task blocked
bacchus release TASK-42 --status blocked

# Failed - discard worktree, reset task to open
bacchus release TASK-42 --status failed
```

## Session Management

Sessions enable stop hooks that prevent premature exit:

```bash
# Start agent session (blocks until task closed)
bacchus session start agent --task-id TASK-42

# Start orchestrator session (blocks while work remains)
bacchus session start orchestrator --max-concurrent 3

# Check session state
bacchus session status

# Clear session to exit
bacchus session stop
```

Session state is stored in `.bacchus/session.json`.

## Stale Detection

Find and cleanup abandoned claims:

```bash
# List stale claims (>30 min old)
bacchus stale --minutes 30

# Auto-cleanup
bacchus stale --minutes 30 --cleanup
```

## Task Definition

Tasks are defined in YAML and imported to SQLite:

```yaml
# .bacchus/tasks.yaml
version: 1

tasks:
  - id: AUTH-001
    title: "Implement user authentication"
    description: "Add JWT-based auth"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies:
        - "src/auth/handler.rs::AuthHandler"
      creates:
        - "src/auth/middleware.rs"

  - id: API-002
    title: "Add rate limiting"
    priority: 2
    depends_on: [AUTH-001]
    footprint:
      modifies: ["src/middleware/mod.rs::*"]
```

Import with: `bacchus task import --epic-id MY-EPIC`

## Directory Structure

```
project/
├── .bacchus/
│   ├── bacchus.db          # SQLite database (tasks, claims, metrics)
│   ├── tasks.yaml          # Task definitions (import source)
│   ├── session.json        # Active session state
│   └── worktrees/
│       ├── TASK-42/        # Agent 1's isolated worktree
│       └── TASK-43/        # Agent 2's isolated worktree
```

## Stop Hook Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    ORCHESTRATOR MODE                         │
│  Spawns agents for ready tasks                              │
│  Blocks while: ready tasks exist OR agents active           │
│  Approves when: all work done or blocked                    │
├─────────────────────────────────────────────────────────────┤
│   ┌─────────┐   ┌─────────┐   ┌─────────┐                  │
│   │ Agent 1 │   │ Agent 2 │   │ Agent 3 │                  │
│   │ TASK-A  │   │ TASK-B  │   │ TASK-C  │                  │
│   └─────────┘   └─────────┘   └─────────┘                  │
│                                                              │
│  AGENT MODE                                                  │
│  Blocks while: assigned task not closed                     │
│  Approves when: task status == "closed"                     │
└─────────────────────────────────────────────────────────────┘
```

## Supported Languages (Symbol Indexing)

- TypeScript / JavaScript
- Python
- Go
- Rust

## License

MIT
