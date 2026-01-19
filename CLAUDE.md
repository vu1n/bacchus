# CLAUDE.md - Bacchus Development Guide

## Overview

Bacchus is a worktree-based coordination CLI for multi-agent work on codebases. It provides isolated git worktrees for parallel agent work with built-in task management via `.bacchus/tasks.yaml`.

**Key concepts:**
- **tasks** = What needs to be done (defined in `.bacchus/tasks.yaml`)
- **claims** = Who's doing what right now (worktrees, sessions)

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Claude Code Plugin                        │
│  ~/.claude/plugins/bacchus/                                  │
│  ├── hooks/stop-router.sh  → bacchus session check          │
│  └── commands/*.md         → /bacchus-agent, /bacchus-orchestrate
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                     bacchus CLI (Rust)                       │
│  ├── Task management (list/show/validate/init)              │
│  ├── Session management (start/stop/status/check)           │
│  ├── Coordination (next/claim/release/stale)                │
│  ├── Symbol indexing (index/symbols)                        │
│  └── Context generation                                      │
└─────────────────────────────────────────────────────────────┘
         │                              │
         ▼                              ▼
┌──────────────────────┐    ┌─────────────────────┐
│ .bacchus/            │    │ git                 │
│ ├── tasks.yaml       │    │ Worktrees           │
│ ├── bacchus.db       │    │ Branches            │
│ ├── session.json     │    │ Merges              │
│ └── worktrees/       │    └─────────────────────┘
└──────────────────────┘
```

## Source Code Structure

```
src/
├── main.rs              # CLI entry point, command routing
├── cli/mod.rs           # Clap command definitions
├── tasks.rs             # YAML-based task management
├── worktree.rs          # Git worktree operations
├── db/                  # SQLite database (claims, symbols)
├── indexer/             # Tree-sitter symbol extraction
├── updater.rs           # Self-update functionality
└── tools/
    ├── mod.rs           # Tool exports
    ├── task_commands.rs # Task list/show/validate/init
    ├── session.rs       # Session management for stop hooks
    ├── claim.rs         # Claim specific task
    ├── next.rs          # Claim next ready task
    ├── release.rs       # Release task (merge/cleanup)
    ├── stale.rs         # Find/cleanup abandoned claims
    ├── list.rs          # List active claims
    ├── abort.rs         # Abort merge conflict
    ├── resolve.rs       # Resolve merge conflict
    ├── symbols.rs       # Symbol search
    └── context/         # Context generation
```

## Key Modules

### Task Management (`src/tasks.rs`)

Manages tasks via `.bacchus/tasks.yaml`:

```rust
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: i32,           // Lower = higher priority (default: 5)
    pub status: String,          // open | in_progress | blocked | closed
    pub depends_on: Vec<String>, // Task IDs that must be closed first
    pub footprint: TaskFootprint,
}

pub struct TaskFootprint {
    pub modifies: Vec<String>,   // Symbols this task will change
    pub creates: Vec<String>,    // New files to create
}

// Key functions:
pub fn load_tasks(workspace_root) -> Result<Vec<Task>>
pub fn get_ready_tasks(workspace_root) -> Result<Vec<Task>>
pub fn update_task_status(workspace_root, task_id, status) -> Result<()>
```

**Ready calculation**: A task is ready when:
1. `status == "open"`
2. All tasks in `depends_on` have `status == "closed"`
3. No footprint collision with in-progress tasks

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

### Worktree Operations (`src/worktree.rs`)

Git worktree management:

```rust
pub fn create_worktree(task_id, workspace_root) -> Result<WorktreeInfo>
pub fn remove_worktree(task_id, workspace_root, force) -> Result<()>
pub fn merge_worktree(task_id, workspace_root, target_branch) -> Result<()>
```

## Task YAML Format

```yaml
version: 1

tasks:
  - id: AUTH-001
    title: "Implement user authentication"
    description: "Add JWT-based auth to the API"
    priority: 1                    # Lower = higher priority (default: 5)
    status: open                   # open | in_progress | blocked | closed
    depends_on: []                 # Task IDs that must be closed first
    footprint:
      modifies:                    # Symbols this task will change
        - "src/auth/handler.rs::AuthHandler"
        - "src/auth/jwt.rs::*"     # Glob: all symbols in file
      creates:                     # New files (virtual footprint)
        - "src/auth/middleware.rs"

  - id: RATE-002
    title: "Add rate limiting"
    priority: 2
    status: open
    depends_on: [AUTH-001]         # Blocked until AUTH-001 is closed
    footprint:
      modifies: ["src/middleware/mod.rs::*"]
      creates: ["src/middleware/rate_limit.rs"]
```

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
        │       ├─► closed → approve (clear session)
        │       └─► not closed → block
        │
        └─► Orchestrator mode:
            ├─► ready tasks + capacity → block (spawn agents)
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
CREATE TABLE tasks_v2 (
    id TEXT PRIMARY KEY,
    epic_id TEXT NOT NULL REFERENCES epics(id),
    title TEXT NOT NULL,
    description TEXT,
    priority INTEGER NOT NULL DEFAULT 5,
    status TEXT NOT NULL DEFAULT 'draft',  -- draft | open | in_progress | blocked | closed
    claimed_by TEXT,                        -- agent_id who claimed
    claimed_at INTEGER,                     -- Unix timestamp ms
    lease_expires_at INTEGER,
    heartbeat_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER                      -- Soft delete
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
| `BACCHUS_WORKTREES` | Override worktrees directory |

## Error Handling

- **Stop hooks fail-open**: If bacchus errors, hooks approve exit (never trap user)
- **Claim validates readiness**: Must be in ready list unless `--force`
- **Merge conflicts**: Return structured error, user can resolve/abort

## Critical: Worktree CWD Footgun

**Never change the main session's working directory to a worktree.**

Worktrees are ephemeral - they get deleted on `bacchus release`. If your shell's cwd points to a deleted worktree, all subsequent bash commands will fail with "no such file or directory".

```bash
# BAD - changes cwd to ephemeral directory
cd .bacchus/worktrees/TASK-42
git status
# ... worktree gets deleted ...
# Shell is now broken!

# GOOD - use -C flag or absolute paths
git -C .bacchus/worktrees/TASK-42 status
git -C .bacchus/worktrees/TASK-42 add .
git -C .bacchus/worktrees/TASK-42 commit -m "msg"
```

**Mitigations:**
1. Always use `git -C <worktree>` instead of `cd <worktree> && git`
2. Sub-agents spawned via Task tool are isolated - their cwd dying doesn't affect parent
3. Before session end/summary, verify cwd is the main repo root
4. Run `git worktree prune` to clean stale worktree refs

## Dependencies

- **Required**: `git`
- **Build**: Rust toolchain, tree-sitter
- **Runtime**: SQLite (bundled via rusqlite)
