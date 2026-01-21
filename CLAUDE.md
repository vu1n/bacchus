# CLAUDE.md - Bacchus Development Guide

## Overview

Bacchus is a workspace-based coordination CLI for multi-agent work on codebases. It provides isolated jj workspaces for parallel agent work with SQLite-based task management.

**Key concepts:**
- **epics** = High-level work containers (groups of related tasks)
- **tasks** = What needs to be done (stored in SQLite, can import from YAML)
- **claims** = Who's doing what right now (task.claimed_by in SQLite)
- **workspaces** = Isolated jj workspaces for each task

## Architecture

### System Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Claude Code Skill                         │
│  ~/.claude/skills/bacchus/                                   │
│  ├── SKILL.md        (workflow, command reference)          │
│  └── archetypes.yaml (specialized agent prompts)            │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                 ~/.claude/settings.json                      │
│  hooks.Stop → bacchus session check                         │
│  (prevents exit until task/orchestration complete)          │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                     bacchus CLI (Rust)                       │
│  ├── Task management (list/show/validate/import)            │
│  ├── Session management (start/stop/status/check)           │
│  ├── Coordination (next/claim/release/stale)                │
│  ├── Symbol indexing (index/symbols)                        │
│  └── Context generation                                      │
└─────────────────────────────────────────────────────────────┘
         │                              │
         ▼                              ▼
┌──────────────────────┐    ┌─────────────────────┐
│ .bacchus/            │    │ jj                  │
│ ├── bacchus.db ────────── │ (SQLite: epics,     │
│ │   tasks, claims)   │    │  Workspaces         │
│ ├── tasks.yaml ──────────▶│  main bookmark      │
│ │   (import only)    │    └─────────────────────┘
│ ├── session.json     │
│ └── workspaces/      │
└──────────────────────┘
```

### Agent vs Orchestrator Roles

Bacchus separates concerns between agents (who do work) and orchestrators (who coordinate):

```
┌───────────────────────────────────────────────────────────────────────┐
│                           ORCHESTRATOR                                 │
│  • Monitors task queue for ready work                                 │
│  • Spawns agent subprocesses for ready tasks                          │
│  • Picks up ready_for_release tasks and merges them                   │
│  • Handles conflicts by marking needs_resolution                      │
│  • Advances main bookmark only after successful rebase                │
└───────────────────────────────────────────────────────────────────────┘
        │                    │                    │
        │ spawn              │ spawn              │ spawn
        ▼                    ▼                    ▼
┌───────────────┐    ┌───────────────┐    ┌───────────────┐
│   AGENT 1     │    │   AGENT 2     │    │   AGENT 3     │
│   AUTH-001    │    │   AUTH-002    │    │   AUTH-003    │
│               │    │               │    │               │
│ • claim task  │    │ • claim task  │    │ • claim task  │
│ • work in ws  │    │ • work in ws  │    │ • work in ws  │
│ • release     │    │ • release     │    │ • release     │
│   (mark ready)│    │   (mark ready)│    │   (mark ready)│
└───────────────┘    └───────────────┘    └───────────────┘
        │                    │                    │
        └────────────────────┴────────────────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │ ready_for_release│
                    │     queue        │
                    └────────┬────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │  ORCHESTRATOR   │
                    │  picks up and   │
                    │  rebases onto   │
                    │     main        │
                    └─────────────────┘
```

### Release Flow (Detailed)

```
Agent completes work
        │
        ▼
bacchus release TASK-42 --status done
        │
        ├─► Validates single commit in workspace
        ├─► Checks for existing conflicts
        ├─► Records commit ID in ready_commit_id
        └─► Sets status = ready_for_release
                    │
                    ▼
        ┌───────────────────────┐
        │  Orchestrator polls   │
        │  for ready_for_release│
        │  tasks periodically   │
        └───────────┬───────────┘
                    │
                    ▼
        ┌───────────────────────┐
        │  start_task_release() │
        │  • status = releasing │
        │  • jj rebase onto main│
        │  • record new commit  │
        └───────────┬───────────┘
                    │
          ┌─────────┴─────────┐
          │                   │
     No conflicts        Has conflicts
          │                   │
          ▼                   ▼
    ┌───────────┐      ┌─────────────────┐
    │ advance   │      │ mark_task_      │
    │ main      │      │ needs_resolution│
    │ bookmark  │      └────────┬────────┘
    └─────┬─────┘               │
          │                     ▼
          ▼              Agent resolves with
    ┌───────────┐        jj resolve, then
    │ complete_ │        bacchus resolve
    │ release() │               │
    │ • closed  │               │
    │ • cleanup │        (loops back to
    │   workspace│        ready_for_release)
    └───────────┘
```

## Source Code Structure

```
src/
├── main.rs              # CLI entry point, command routing
├── cli/mod.rs           # Clap command definitions
├── tasks.rs             # SQLite task management + YAML import
├── epics.rs             # Epic management
├── workspace.rs         # jj workspace operations
├── db/                  # SQLite database (schema, connection)
├── indexer/             # Tree-sitter symbol extraction
├── updater.rs           # Self-update functionality
└── tools/
    ├── mod.rs           # Tool exports
    ├── task_commands.rs # Task list/show/validate/import
    ├── session.rs       # Session management for stop hooks
    ├── claim.rs         # Claim specific task
    ├── next.rs          # Claim next ready task
    ├── release.rs       # Release task (mark ready for release)
    ├── review.rs        # Advisory review checks before release
    ├── stale.rs         # Find/cleanup abandoned claims
    ├── list.rs          # List active claims
    ├── abort.rs         # Reset task from needs_resolution
    ├── resolve.rs       # Mark resolved task ready for release
    ├── symbols.rs       # Symbol search
    ├── archetypes.rs    # Agent archetype selection
    ├── eval.rs          # Eval metrics tracking
    └── context/         # Context generation
```

## Type vs Archetype

Tasks have two orthogonal classifications:

**task_type** (PM workflow):
| Type | Purpose |
|------|---------|
| `bug_fix` | Fixing defects |
| `feature` | Adding new functionality |
| `refactor` | Restructuring without changing behavior |
| `test` | Adding/improving tests |
| `docs` | Documentation |
| `infra` | CI/CD, deployment, infrastructure |
| `generic` | General work (default) |

**archetype** (Agent specialization):
| Archetype | Focus |
|-----------|-------|
| `frontend` | UI/UX, components, CSS, accessibility |
| `backend` | APIs, auth, validation, error handling |
| `data` | Pipelines, SQL, schemas, ETL |
| `test` | Coverage, fixtures, e2e, mocks |
| `infra` | CI/CD, containers, cloud, monitoring |
| `review` | Quality, patterns, correctness |
| `security` | Vulnerabilities, OWASP, secrets |
| `generic` | General development (default) |

**Key design decision**: Archetype is explicitly set by the planner (no inference). Full definitions with prompts are in `archetypes.yaml`.

## Key Modules

### Task Management (`src/tasks.rs`)

All task state lives in SQLite. YAML is import-only format.

```rust
// SQLite task (primary)
pub struct SqliteTask {
    pub id: String,
    pub epic_id: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: i32,
    pub status: String,          // draft | open | in_progress | ready_for_release | releasing | needs_resolution | blocked | closed
    pub task_type: SqliteTaskType, // PM workflow: bug_fix | feature | refactor | test | docs | infra | generic
    pub archetype: String,         // Agent specialization: frontend | backend | data | test | infra | review | security | generic
    pub claimed_by: Option<String>,
    pub claimed_at: Option<i64>,
    pub ready_commit_id: Option<String>,    // jj commit ID when marked ready
    pub release_commit_id: Option<String>,  // jj commit ID after rebase
    pub release_started_at: Option<i64>,
}

// Key functions:
pub fn import_yaml_tasks(workspace_root, epic_id) -> Result<ImportResult>
pub fn claim_sqlite_task(task_id, agent_id) -> Result<SqliteTask>
pub fn claim_next_sqlite_task(agent_id) -> Result<Option<SqliteTask>>
pub fn release_sqlite_task(task_id, new_status) -> Result<()>
pub fn get_sqlite_task(task_id) -> Result<SqliteTask>

// jj workflow functions:
pub fn mark_task_ready_for_release(task_id, agent_id, commit_id) -> Result<()>
pub fn start_task_release(task_id, release_commit_id) -> Result<()>
pub fn complete_task_release(task_id) -> Result<()>
pub fn mark_task_needs_resolution(task_id, conflict_files) -> Result<()>
pub fn get_tasks_ready_for_release() -> Result<Vec<SqliteTask>>
```

**Ready calculation**: A task is ready when:
1. `status == "open"`
2. All tasks in `depends_on` have `status == "closed"`
3. No footprint collision with in-progress tasks

### jj Workspace Operations (`src/workspace.rs`)

jj workspace management for isolated agent work:

```rust
pub fn create_workspace(workspace_root, task_id) -> Result<WorkspaceInfo>
pub fn remove_workspace(workspace_root, task_id, force) -> Result<()>
pub fn validate_single_commit(workspace_root, task_id) -> Result<String>
pub fn has_conflicts(workspace_root, task_id) -> Result<bool>
pub fn get_conflict_files(workspace_root, task_id) -> Result<Vec<String>>

// Orchestrator-only operations:
pub fn rebase_workspace_onto_main(workspace_root, task_id) -> Result<ReleaseResult>
pub fn advance_main_bookmark(workspace_root, commit_id) -> Result<()>
pub fn complete_release(workspace_root, task_id) -> Result<()>
pub fn is_commit_in_main(workspace_root, commit_id) -> Result<bool>
```

**Key design decisions:**
- **Orchestrator-only release**: Only orchestrator advances main bookmark
- **Single-commit per task**: Validated before marking ready
- **Task ID = Workspace name**: For safe jj revset queries

### Session Management (`src/tools/session.rs`)

Manages `.bacchus/session.json` for stop hook integration:

```rust
pub struct Session {
    pub mode: String,           // "agent" | "orchestrator"
    pub task_id: Option<String>, // For agent mode
    pub max_concurrent: Option<i32>, // For orchestrator mode
    pub started_at: String,
}

// Key functions:
pub fn start_session(mode, task_id, max_concurrent) -> Result<String>
pub fn stop_session() -> Result<String>
pub fn session_status() -> Result<Value>
pub fn check_session() -> HookCheckOutput  // For stop hook
```

**Workspace root detection priority:**
1. `CLAUDE_PROJECT_DIR` env var (set by Claude Code for hooks)
2. Walk up from CWD looking for `.bacchus` or `.git`

## Task Status Lifecycle

```
                   ┌─────────┐
                   │  draft  │
                   └────┬────┘
                        │ (import/open)
                        ▼
   ┌─────────────► ┌─────────┐ ◄─────────────┐
   │               │  open   │               │
   │               └────┬────┘               │
   │                    │ (claim)            │
   │                    ▼                    │
   │ (failed)      ┌─────────────┐           │ (reset from
   └───────────────│ in_progress │───────┐   │  needs_resolution)
                   └──────┬──────┘       │   │
                          │ (done)       │   │
                          ▼              │   │
                   ┌──────────────────┐  │   │
                   │ ready_for_release│──┼───┘
                   └────────┬─────────┘  │
                            │ (orchestrator)
                            ▼
                   ┌──────────────┐       │
                   │  releasing   │       │
                   └──────┬───────┘       │
                   ┌──────┴──────┐        │
             success│            │conflicts│
                   ▼             ▼         │
             ┌─────────┐  ┌────────────────┐
             │ closed  │  │needs_resolution│
             └─────────┘  └────────────────┘
```

## Agent Release Workflow

1. Agent completes work in workspace
2. Agent runs `bacchus release <task_id> --status done`
   - Validates single commit above main
   - Checks for conflicts
   - Marks task `ready_for_release` with commit ID
3. Orchestrator picks up ready tasks
4. Orchestrator rebases onto main, advances bookmark
5. Task marked `closed`, workspace cleaned up

## Task YAML Format (Import Only)

YAML is used for bulk task definition. Import with `bacchus task import --epic-id <EPIC>`.

```yaml
version: 1

tasks:
  - id: AUTH-001
    title: "Implement user authentication"
    description: "Add JWT-based auth to the API"
    type: feature                  # PM workflow: bug_fix | feature | refactor | test | docs | infra | generic
    archetype: backend             # Agent expertise: frontend | backend | data | test | infra | review | security | generic
    priority: 1                    # Lower = higher priority (default: 5)
    status: open                   # open | in_progress | blocked | closed
    depends_on: []                 # Task IDs that must be closed first
    footprint:
      modifies:                    # Symbols this task will change
        - "src/auth/handler.rs::AuthHandler"
        - "src/auth/jwt.rs::*"     # Glob: all symbols in file
      creates:                     # New files (virtual footprint)
        - "src/auth/middleware.rs"

  - id: AUTH-002
    title: "Add login form"
    type: feature
    archetype: frontend
    priority: 2
    depends_on: [AUTH-001]
    footprint:
      creates: ["src/components/LoginForm.tsx"]

  - id: AUTH-001-SEC
    title: "Security review of authentication"
    type: feature                  # It's a feature task (the review itself)
    archetype: security            # But needs security expertise
    priority: 3
    depends_on: [AUTH-001]
```

After import, all operations (claim, release, status changes) happen in SQLite.

## Stop Hook Flow

```
Claude tries to exit
        │
        ▼
stop-router.sh runs
        │
        ▼
bacchus session check
        │
        ├─► No session → approve
        │
        ├─► Agent mode:
        │   └─► Check task status
        │       ├─► closed/ready_for_release → approve (clear session)
        │       └─► not closed → block
        │
        └─► Orchestrator mode:
            ├─► ready tasks + capacity → block (spawn agents)
            ├─► tasks ready_for_release → block (release them)
            ├─► active claims → block (wait)
            ├─► in_progress without claims → block (orphaned)
            └─► all done/blocked → approve (clear session)
```

## Development

### Build

```bash
cargo build           # Debug build
cargo build --release # Release build
cargo test -- --test-threads=1  # Run tests (sequential due to global DB pool)
```

### Local Testing

```bash
# Test task commands
./target/debug/bacchus task init
./target/debug/bacchus task list
./target/debug/bacchus task list --ready

# Test session commands
./target/debug/bacchus session start agent --task-id "TEST-123"
./target/debug/bacchus session status
./target/debug/bacchus session check
./target/debug/bacchus session stop

# Test with stop hook
echo "" | bash plugin/hooks/stop-router.sh
```

### Install Local Build

```bash
cp ./target/release/bacchus ~/.local/bin/bacchus
```

## Release Process

1. **Bump version** in `Cargo.toml`
2. **Commit** changes
3. **Create tag**: `git tag -a v0.X.0 -m "v0.X.0: Description"`
4. **Push tag**: `git push origin v0.X.0`
5. **GitHub Actions** automatically:
   - Builds binaries for linux-x86_64, linux-aarch64, darwin-x86_64, darwin-aarch64
   - Creates GitHub release with binaries

The install script downloads from the release matching the latest tag.

## Database Schema

All task state lives in SQLite (`bacchus.db`). YAML is import-only.

### Tasks Table

```sql
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    epic_id TEXT NOT NULL REFERENCES epics(id),
    title TEXT NOT NULL,
    description TEXT,
    priority INTEGER NOT NULL DEFAULT 5,
    status TEXT NOT NULL DEFAULT 'draft',  -- draft | open | in_progress | ready_for_release | releasing | needs_resolution | blocked | closed
    task_type TEXT NOT NULL DEFAULT 'generic',  -- PM workflow type
    archetype TEXT NOT NULL DEFAULT 'generic',  -- Agent specialization
    claimed_by TEXT,                        -- agent_id who claimed
    claimed_at INTEGER,                     -- Unix timestamp ms
    ready_commit_id TEXT,                   -- jj commit ID when marked ready
    release_commit_id TEXT,                 -- jj commit ID after rebase
    release_started_at INTEGER,             -- When orchestrator started release
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,                     -- Soft delete
    CHECK (task_type IN ('bug_fix', 'feature', 'refactor', 'test', 'docs', 'infra', 'generic')),
    CHECK (archetype IN ('frontend', 'backend', 'data', 'test', 'infra', 'review', 'security', 'generic'))
);
```

### Epics Table

```sql
CREATE TABLE epics (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'open',  -- open | planning | active | closed
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

### Symbols Table

```sql
CREATE TABLE symbols (
    id INTEGER PRIMARY KEY,
    file TEXT NOT NULL,
    fq_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    span_start_line INTEGER NOT NULL,
    span_end_line INTEGER NOT NULL,
    line_count INTEGER NOT NULL,
    hash TEXT NOT NULL,
    docstring TEXT,
    language TEXT NOT NULL
);
```

### Supporting Tables

- `task_dependencies` - Task dependency edges (same-epic enforced by trigger)
- `task_footprints` - Normalized footprint patterns for overlap detection
- `agent_messages` - Pull-based agent communication queue
- `symbols_fts` - FTS5 full-text search for symbols

## Environment Variables

| Variable | Description |
|----------|-------------|
| `CLAUDE_PROJECT_DIR` | Set by Claude Code, used for workspace root detection |
| `BACCHUS_DB_PATH` | Override database location |
| `BACCHUS_WORKSPACES` | Override workspaces directory |

## Error Handling

- **Stop hooks fail-open**: If bacchus errors, hooks approve exit (never trap user)
- **Claim validates readiness**: Must be in ready list unless `--force`
- **Release conflicts**: Return structured error, orchestrator handles resolution

## Critical: Workspace CWD Footgun

**Never change the main session's working directory to a workspace.**

Workspaces are ephemeral - they get deleted on `bacchus release`. If your shell's cwd points to a deleted workspace, all subsequent bash commands will fail with "no such file or directory".

```bash
# BAD - changes cwd to ephemeral directory
cd .bacchus/workspaces/TASK-42
jj status
# ... workspace gets deleted ...
# Shell is now broken!

# GOOD - use -R flag or absolute paths
jj -R .bacchus/workspaces/TASK-42 status
jj -R .bacchus/workspaces/TASK-42 commit -m "msg"
```

**Mitigations:**
1. Always use `jj -R <workspace>` instead of `cd <workspace> && jj`
2. Sub-agents spawned via Task tool are isolated - their cwd dying doesn't affect parent
3. Before session end/summary, verify cwd is the main repo root

## Eval Metrics Framework

Bacchus tracks agent performance metrics for analysis and improvement.

### Event Types

| Event | Description |
|-------|-------------|
| `started` | Agent claimed the task |
| `completed` | Task marked ready_for_release |
| `failed` | Task released with status=failed |
| `rework` | Task was re-claimed after previous completion |

### Metrics Table

```sql
CREATE TABLE task_eval_metrics (
    id INTEGER PRIMARY KEY,
    task_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    event_type TEXT NOT NULL,  -- started | completed | failed | rework
    event_data TEXT,           -- JSON with additional context
    created_at INTEGER NOT NULL
);
```

### Commands

```bash
# View completion metrics for last 7 days
bacchus eval

# Filter by epic
bacchus eval --epic AUTH

# Custom time range
bacchus eval --days 30
```

### Metrics Reported

- **Completion rate**: `completed / (completed + failed)`
- **Rework rate**: `rework / completed`
- **Average time to complete**: Mean duration from started to completed
- **Tasks per agent**: Distribution of work across agents

## jj-Specific Notes

### Why jj over git worktrees?

1. **Non-blocking conflicts**: jj allows commits with conflicts. Agents can continue working while conflicts exist elsewhere.
2. **Auto-snapshot**: No explicit `git add` needed. Changes are automatically tracked.
3. **Simpler rebasing**: `jj rebase` is more intuitive than git's.
4. **Workspace isolation**: jj workspaces are lighter-weight than git worktrees.

### jj Commands Reference

| Task | Command |
|------|---------|
| Check status | `jj -R <workspace> status` |
| View changes | `jj -R <workspace> diff` |
| Describe (commit msg) | `jj -R <workspace> describe -m "msg"` |
| View log | `jj -R <workspace> log` |
| Resolve conflicts | `jj -R <workspace> resolve` |
| Rebase onto main | `jj -R <workspace> rebase -d main` |

### Bookmark vs Branch

jj uses **bookmarks** instead of branches. The `main` bookmark points to the integration point where all work is merged.

```bash
# View bookmarks
jj bookmark list

# Advance main to commit
jj bookmark set main -r <commit_id>
```

## Dependencies

- **Required**: `jj` (Jujutsu VCS) v0.20+
- **Build**: Rust toolchain, tree-sitter
- **Runtime**: SQLite (bundled via rusqlite)
