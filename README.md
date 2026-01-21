# Bacchus

Workspace-based coordination CLI for multi-agent work on codebases using [jj (Jujutsu)](https://martinvonz.github.io/jj/).

Bacchus helps AI agents coordinate when working on the same codebase by:
- **Workspace isolation** - each agent works in its own jj workspace
- **Task management** - SQLite-based task tracking with dependencies and footprints
- **Session management** - stop hooks keep agents working until tasks complete
- **Orchestrator-driven release** - agents mark work ready, orchestrator handles merging
- **Non-blocking conflicts** - jj allows conflict detection without blocking work

## Installation

```bash
curl -fsSL https://raw.githubusercontent.com/vu1n/bacchus/main/scripts/install.sh | bash
```

This installs:
- `bacchus` binary to `~/.local/bin/`
- Claude Code plugin to `~/.claude/plugins/bacchus/`

### Prerequisites

- [jj (Jujutsu)](https://martinvonz.github.io/jj/latest/install/) v0.20+
- git (jj uses git backend)

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
# Initialize jj repo (if not already)
jj git init --colocate  # or jj init

# Initialize tasks
bacchus task init
# Edit .bacchus/tasks.yaml to define tasks
bacchus task import --epic-id MY-EPIC

# Get next ready task (creates workspace, claims it)
bacchus next agent-1

# Or claim a specific task
bacchus claim TASK-42 agent-1

# Work in the isolated workspace (use -R flag, don't cd)
jj -R .bacchus/workspaces/TASK-42 status
# ... make changes (auto-snapshotted by jj) ...
jj -R .bacchus/workspaces/TASK-42 describe -m "Implement feature"

# Mark ready for release (orchestrator will merge)
bacchus release TASK-42 --status done
```

## Quickstart Guide

This guide walks through setting up Bacchus for a multi-agent workflow on an existing project.

### Step 1: Initialize Your Repository

Bacchus uses jj (Jujutsu) for version control. If your project uses git, initialize jj in colocated mode:

```bash
cd your-project
jj git init --colocate

# Set up user config for jj
jj config set --repo user.name "Your Name"
jj config set --repo user.email "you@example.com"

# Create main bookmark (like a branch)
jj bookmark create main -r @
jj describe -m "Initial commit"
jj new
```

### Step 2: Define Your Tasks

Create a task definition file:

```bash
bacchus task init
```

Edit `.bacchus/tasks.yaml`:

```yaml
version: 1

tasks:
  - id: AUTH-001
    title: "Add user login endpoint"
    description: |
      Create POST /api/login that:
      - Validates credentials
      - Returns JWT token
      - Handles errors appropriately
    priority: 1
    status: open
    depends_on: []
    footprint:
      creates:
        - "src/routes/login.rs"
      modifies:
        - "src/routes/mod.rs::*"

  - id: AUTH-002
    title: "Add authentication middleware"
    description: "Protect routes with JWT validation"
    priority: 2
    status: open
    depends_on: [AUTH-001]  # Must complete login first
    footprint:
      creates:
        - "src/middleware/auth.rs"

  - id: AUTH-003
    title: "Add logout endpoint"
    description: "POST /api/logout to invalidate tokens"
    priority: 2
    status: open
    depends_on: [AUTH-001]  # Can run parallel with AUTH-002
    footprint:
      creates:
        - "src/routes/logout.rs"
```

Import tasks to SQLite:

```bash
bacchus task import --epic-id AUTH

# Verify import
bacchus task list
bacchus task list --ready  # Shows AUTH-001 (others blocked)
```

### Step 3: Work on a Task (Agent Mode)

Claim your first task:

```bash
bacchus claim AUTH-001 agent-1
```

Output:
```json
{
  "success": true,
  "task_id": "AUTH-001",
  "title": "Add user login endpoint",
  "workspace_path": ".bacchus/workspaces/AUTH-001"
}
```

Work in the isolated workspace:

```bash
# Check workspace status
jj -R .bacchus/workspaces/AUTH-001 status

# Make your changes (files are auto-tracked by jj)
# Create src/routes/login.rs, etc.

# Describe your change (like a commit message)
jj -R .bacchus/workspaces/AUTH-001 describe -m "Implement login endpoint with JWT"

# View what you've done
jj -R .bacchus/workspaces/AUTH-001 log
jj -R .bacchus/workspaces/AUTH-001 diff
```

> **Warning**: Never `cd` into the workspace! Use `jj -R <path>` instead. Workspaces are deleted on release.

### Step 4: Release Your Work

When done, mark the task ready for release:

```bash
bacchus release AUTH-001 --status done
```

Output:
```json
{
  "success": true,
  "task_id": "AUTH-001",
  "status": "ready_for_release",
  "commit_id": "abc123...",
  "message": "Task AUTH-001 marked ready for release. Orchestrator will merge."
}
```

Check what's now available:

```bash
bacchus task list --ready
# Now shows AUTH-002 and AUTH-003 (AUTH-001 unblocked them)
```

### Step 5: Handle Conflicts (If Needed)

If the orchestrator detects conflicts during merge:

```bash
# Task will be marked needs_resolution
bacchus task show AUTH-001

# Resolve conflicts in your workspace
jj -R .bacchus/workspaces/AUTH-001 resolve

# Mark resolved and ready again
bacchus resolve AUTH-001
```

### Common Scenarios

**Abandoning work:**
```bash
# Discard changes and reset task to open
bacchus release AUTH-001 --status failed
```

**Blocked on something:**
```bash
# Keep workspace but mark task blocked
bacchus release AUTH-001 --status blocked
```

**Finding stale work:**
```bash
# List claims older than 30 minutes
bacchus stale --minutes 30

# Clean them up automatically
bacchus stale --minutes 30 --cleanup
```

**Using with Claude Code plugin:**
```bash
# Start agent session (stop hook keeps you working)
/bacchus-agent AUTH-001

# Or run as orchestrator (spawns multiple agents)
/bacchus-orchestrate --max_concurrent 3
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
| `next <agent_id>` | Get next ready task, create workspace, claim it |
| `claim <task_id> <agent_id> [--force]` | Claim specific task (must be ready unless --force) |
| `release <task_id> --status done\|blocked\|failed` | Mark task ready for release |
| `stale [--minutes N] [--cleanup]` | Find/cleanup abandoned claims |
| `list` | List all active claims |
| `resolve <task_id>` | Mark task ready after resolving conflicts |
| `abort <task_id>` | Reset from needs_resolution to in_progress |

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
| `status` | Show claims, orphaned workspaces, broken claims |
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
claim/next → work in workspace → release (mark ready) → orchestrator merges
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
  "workspace_path": ".bacchus/workspaces/TASK-42"
}
```

### 2. Do Work

Work in the jj workspace. Changes are auto-snapshotted - no explicit add/commit needed.

> **Warning**: Never `cd` into a workspace. Use `jj -R` instead - workspaces are ephemeral and get deleted on release.

```bash
# Check status
jj -R .bacchus/workspaces/TASK-42 status

# Describe your change (like a commit message)
jj -R .bacchus/workspaces/TASK-42 describe -m "Implement auth"

# View your changes
jj -R .bacchus/workspaces/TASK-42 diff
```

### 3. Release

```bash
# Success - mark ready for release (orchestrator will merge)
bacchus release TASK-42 --status done

# Blocked - keep workspace, mark task blocked
bacchus release TASK-42 --status blocked

# Failed - discard workspace, reset task to open
bacchus release TASK-42 --status failed
```

### 4. Orchestrator Merges (Automatic)

When you mark a task ready, the orchestrator:
1. Rebases your commit onto current main
2. If conflicts: marks task `needs_resolution` (you fix with `jj resolve`)
3. If clean: advances main bookmark and closes task

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
├── .jj/                    # jj repository data
├── .bacchus/
│   ├── bacchus.db          # SQLite database (tasks, claims, metrics)
│   ├── tasks.yaml          # Task definitions (import source)
│   ├── session.json        # Active session state
│   └── workspaces/
│       ├── TASK-42/        # Agent 1's jj workspace
│       └── TASK-43/        # Agent 2's jj workspace
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
