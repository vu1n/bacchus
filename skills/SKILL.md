---
name: bacchus
description: Multi-agent coordination CLI for codebases. Use when orchestrating parallel agents, claiming tasks, detecting symbol conflicts, or notifying stakeholders of breaking changes. Invoke when user mentions coordination, parallel agents, multiple agents, task claiming, or conflict detection.
---

# Bacchus - Worktree-Based Agent Coordination

Lightweight coordination for parallel agent work. Uses git worktrees for isolation with SQLite-based task management.

## Installation

```bash
curl -fsSL https://raw.githubusercontent.com/vu1n/bacchus/main/scripts/install.sh | bash
```

## Core Workflow

```
next/claim → work in worktree → release
```

### 1. Get Work

**Option A: Claim next ready task**
```bash
bacchus next agent-1
```
Queries SQLite for ready tasks and claims the highest-priority one.

**Option B: Claim a specific task**
```bash
bacchus claim PROJ-42 agent-1
```
Claims a specific task by ID (useful when assigning specific tasks to agents).

Both commands:
- Create isolated worktree at `.bacchus/worktrees/{task_id}/`
- Record the claim in SQLite (status → in_progress)
- Return worktree path and branch name

Output:
```json
{
  "success": true,
  "task_id": "PROJ-42",
  "title": "Implement user auth",
  "worktree_path": ".bacchus/worktrees/PROJ-42",
  "branch": "bacchus/PROJ-42"
}
```

### 2. Do Work

Work in the worktree. All changes are isolated on branch `bacchus/{task_id}`.

> **Warning**: Never `cd` into a worktree from your main session. Worktrees are ephemeral and get deleted on release. Use `git -C` instead:

```bash
# Use -C flag to operate in worktree without changing cwd
git -C .bacchus/worktrees/PROJ-42 status
git -C .bacchus/worktrees/PROJ-42 add .
git -C .bacchus/worktrees/PROJ-42 commit -m "Implement auth"

# Or spawn a sub-agent (Task tool) that works in the worktree
# Sub-agents are isolated - their cwd dying doesn't affect parent
```

### 3. Release

```bash
# Success - merge to main, cleanup worktree, close task
bacchus release PROJ-42 --status done

# Blocked - keep worktree for later, mark task blocked
bacchus release PROJ-42 --status blocked

# Failed - discard changes, reset task to open
bacchus release PROJ-42 --status failed
```

## Task Management

Tasks are defined in YAML and imported to SQLite:

```bash
# Initialize tasks.yaml template
bacchus task init

# List tasks
bacchus task list
bacchus task list --ready    # Only ready tasks
bacchus task list --status open

# Show task details
bacchus task show PROJ-42

# Import tasks to SQLite
bacchus task import --epic-id MY-EPIC
```

### Task YAML Format

```yaml
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
```

## Stale Detection

Find and cleanup abandoned claims:

```bash
# List stale claims (>30 min old)
bacchus stale --minutes 30

# Auto-cleanup: remove worktrees, reset tasks to open
bacchus stale --minutes 30 --cleanup
```

## List Active Claims

See all active claims and worktrees:

```bash
bacchus list
```

Output:
```json
{
  "claims": [
    {
      "task_id": "PROJ-42",
      "agent_id": "agent-1",
      "worktree_path": ".bacchus/worktrees/PROJ-42",
      "branch_name": "bacchus/PROJ-42",
      "age_minutes": 5
    }
  ],
  "total": 1
}
```

## Code Search

Index and search symbols:

```bash
# Index a directory
bacchus index src/

# Search for symbols
bacchus symbols --pattern "User*" --kind class
bacchus symbols --file "src/auth.ts"
```

## Status

```bash
bacchus status
```

Shows active claims, worktree locations, and indexed symbol count.

## Context

Generate a markdown summary of the current environment to ground the agent:

```bash
bacchus context
bacchus context --task-id PROJ-42
```

- **Global Mode** (no task): Shows all active claims and ready work.
- **Task Mode** (with --task-id): Shows specific objectives, dependencies, footprint, and type-specific guidance.

## Review & Eval

```bash
# Review task before release (advisory checks)
bacchus review PROJ-42
bacchus review PROJ-42 --build-cmd "cargo build" --test-cmd "cargo test"

# View completion metrics
bacchus eval
bacchus eval --epic MY-EPIC --days 30
```

## Merge Conflict Handling

When `release --status done` encounters a conflict:

```bash
# Option 1: Resolve manually, then complete
# (fix conflicts in files, git add them)
bacchus resolve PROJ-42

# Option 2: Abort merge, keep working
bacchus abort PROJ-42

# Option 3: Discard all work
bacchus release PROJ-42 --status failed
```

## Orchestrator Pattern

Main agent has broad context (project overview, all tasks). Sub-agents have focused context (single task + worktree).

### Using Claude Code Task Tool (Primary)

1. **Claim task**:
   ```bash
   bacchus next worker-1
   # Returns: { task_id, title, description, worktree_path }
   ```

2. **Spawn sub-agent**:
   ```
   Task tool:
     subagent_type: "general-purpose"
     prompt: |
       Work in {worktree_path} on task {task_id}: {title}

       {description}

       Commit changes when done.
     run_in_background: true
   ```

3. **Monitor**: `TaskOutput(task_id)`

4. **Release**: `bacchus release {task_id} --status done|failed`

5. **Repeat** as needed - scale up/down based on workload

### Why Sequential Claiming Works

```
bacchus next worker-1  → claims TASK-1, marks in_progress
bacchus next worker-2  → TASK-1 taken, gets TASK-2
bacchus next worker-3  → TASK-1,2 taken, gets TASK-3
```

No race conditions. Each `next` call atomically claims the highest-priority ready task.

### Using Terminal Multiplexer (Human Monitoring)

For visual debugging with zellij or tmux:

```bash
# Each pane runs an isolated agent
zellij run -- claude --print "Work on TASK-1 in .bacchus/worktrees/TASK-1"
zellij run -- claude --print "Work on TASK-2 in .bacchus/worktrees/TASK-2"
```

Each pane shows real-time agent output. Useful for debugging but main agent can't easily read structured results.

## Directory Structure

```
project/
├── .bacchus/
│   ├── bacchus.db          # SQLite database (tasks, claims, metrics)
│   ├── tasks.yaml          # Task definitions (import source)
│   ├── session.json        # Active session state
│   └── worktrees/
│       ├── PROJ-42/        # Isolated worktree for PROJ-42
│       └── PROJ-43/        # Another agent's worktree
```
