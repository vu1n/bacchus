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
- Claude Code skill to `~/.claude/skills/bacchus/`
- Stop hooks in `~/.claude/settings.json`

No additional configuration needed - the skill and hooks are automatically available.

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

## How It Works

Bacchus coordinates multi-agent work through a **plan → orchestrate → execute** flow:

```
┌─────────────────────────────────────────────────────────────────────┐
│  1. PLAN                                                            │
│     User request → /bacchus-planner or /bacchus-architect          │
│     Breaks down work into tasks with dependencies                   │
│     Outputs: .bacchus/tasks.yaml                                    │
├─────────────────────────────────────────────────────────────────────┤
│  2. IMPORT                                                          │
│     bacchus task import --epic-id <EPIC>                           │
│     Loads tasks from YAML into SQLite                               │
│     Calculates ready tasks (no blockers, no footprint conflicts)   │
├─────────────────────────────────────────────────────────────────────┤
│  3. ORCHESTRATE                                                     │
│     /bacchus-orchestrate spawns agents for ready tasks             │
│     Monitors progress, handles merges, manages conflicts            │
├─────────────────────────────────────────────────────────────────────┤
│  4. EXECUTE                                                         │
│     Each agent: claim → work in jj workspace → release             │
│     Orchestrator: rebase → merge to main → close task              │
└─────────────────────────────────────────────────────────────────────┘
```

### How to Prompt Claude

For multi-agent parallel work, tell Claude what you want and mention bacchus:

```
"I want to add user authentication with login, logout, and password reset.
Use bacchus to parallelize this work across multiple agents."
```

Claude will automatically plan the work, import tasks, and spawn agents with appropriate archetypes.

| What you want | What to say |
|---------------|-------------|
| Full automation | "Use bacchus to implement X with N agents" |
| Plan only | "Break down X into tasks for bacchus" |
| Single task | "Work on task TASK-001 with bacchus" |
| Check status | "Show bacchus status" |

**Key roles:**
- **Planner**: Breaks down requests into atomic tasks with dependencies
- **Orchestrator**: Spawns agents, monitors progress, handles merges
- **Agent**: Works on a single task in an isolated workspace (with type-specific archetype)

## Quickstart Guide

This guide walks through setting up Bacchus for a multi-agent workflow.

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

### Step 2: Plan Your Tasks

You can create tasks manually or ask Claude to help plan:

**Option A: Ask Claude to plan (recommended for complex work)**
```
"Break down user authentication into tasks for bacchus"
```
Claude will analyze your request and create tasks with proper dependencies.

**Option B: Create tasks manually**

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

**Full workflow with Claude:**
```
# Tell Claude what you want:
"Implement user authentication with bacchus using 3 agents"

# Claude will:
# 1. Plan: break down into tasks.yaml
# 2. Import: bacchus task import --epic-id AUTH
# 3. Orchestrate: spawn agents and monitor progress
```

**Manual orchestration:**
```bash
# After planning and importing, start orchestrator session
bacchus session start orchestrator --max-concurrent 3
# Then ask Claude to spawn agents for ready tasks
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
| `session start architect --agent-id <id>` | Start architect session |
| `session stop` | Clear session, allow exit |
| `session status` | Show current session state |
| `session check` | Check if exit should be blocked (for hooks) |

### Epic Management

| Command | Description |
|---------|-------------|
| `epic list [--status X]` | List epics (open, planning, active, closed) |
| `epic show <epic_id>` | Show epic details with task counts |
| `epic create --id X --title Y [--description Z]` | Create a new epic |
| `epic assign <epic_id> <agent_id>` | Assign epic to architect for breakdown |

### Archetype Management

| Command | Description |
|---------|-------------|
| `archetype list` | List available agent archetypes |
| `archetype show <name>` | Show archetype details and keywords |
| `archetype prompt <name>` | Get the specialized prompt for an archetype |
| `archetype select <task_id>` | Select best archetype for a task |

### Messaging

| Command | Description |
|---------|-------------|
| `message list [--agent X] [--status Y]` | List agent messages |
| `message send <agent> <type> <payload>` | Send message to agent |

### Symbols

| Command | Description |
|---------|-------------|
| `index <path>` | Index files for symbol search |
| `symbols [--pattern X] [--kind Y]` | Search for symbols |
| `symbols [--file X] [--lang Y]` | Filter by file path or language |
| `symbols [--search X] [--fuzzy]` | Full-text search with fuzzy matching |

### Info

| Command | Description |
|---------|-------------|
| `status` | Show claims, orphaned workspaces, broken claims |
| `context [--task-id X]` | Generate markdown context for agent |
| `workflow` | Print protocol documentation |
| `self-update` | Update bacchus to latest version |
| `check-update` | Check if newer version is available |

## Claude Code Integration

Bacchus integrates with Claude Code through a skill and stop hooks:

### Skill

The bacchus skill (`~/.claude/skills/bacchus/SKILL.md`) provides:
- Workflow guidance for planning, importing, and orchestrating tasks
- Agent archetype prompts for specialized task execution
- Command reference for the bacchus CLI

The skill is automatically loaded when you mention "bacchus" or multi-agent coordination.

### Stop Hooks

Stop hooks in `~/.claude/settings.json` prevent premature exit:
- **Agent mode**: Blocks exit until assigned task is closed
- **Orchestrator mode**: Blocks exit while work remains

### Type vs Archetype

Tasks have two separate classifications:

**Task Type** (PM workflow - what kind of work):
`bug_fix` | `feature` | `refactor` | `test` | `docs` | `infra` | `generic`

**Archetype** (Agent specialization - what expertise):
| Archetype | Focus |
|-----------|-------|
| `frontend` | UI/UX, components, styling |
| `backend` | APIs, auth, validation |
| `data` | Pipelines, SQL, schemas |
| `test` | Coverage, fixtures, e2e |
| `infra` | CI/CD, containers, cloud |
| `review` | Quality, patterns |
| `security` | Vulnerabilities, OWASP |
| `generic` | General development |

Archetypes are explicitly set by the planner - no inference. The orchestrator uses the archetype to load specialized agent prompts.

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
    type: feature              # PM workflow: bug_fix | feature | refactor | test | docs | infra | generic
    archetype: backend         # Agent expertise: frontend | backend | data | test | infra | review | security | generic
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
    type: feature
    archetype: backend
    priority: 2
    depends_on: [AUTH-001]
    footprint:
      modifies: ["src/middleware/mod.rs::*"]

  - id: AUTH-001-SEC
    title: "Security review of auth"
    type: feature              # The review is a feature task
    archetype: security        # But needs security expertise
    priority: 3
    depends_on: [AUTH-001]
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
