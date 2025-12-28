# CLAUDE.md - Bacchus Development Guide

## Overview

Bacchus is a worktree-based coordination CLI for multi-agent work on codebases. It integrates with [beads](https://github.com/vu1n/beads) for task tracking and provides isolated git worktrees for parallel agent work.

**Key concepts:**
- **beads** = What needs to be done (issues, dependencies, status)
- **bacchus** = Who's doing what right now (claims, worktrees, sessions)

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
│  ├── Session management (start/stop/status/check)           │
│  ├── Coordination (next/claim/release/stale)                │
│  ├── Symbol indexing (index/symbols)                        │
│  └── Context generation                                      │
└─────────────────────────────────────────────────────────────┘
         │                    │                    │
         ▼                    ▼                    ▼
┌─────────────┐    ┌──────────────────┐    ┌─────────────┐
│ .bacchus/   │    │ beads CLI (bd)   │    │ git         │
│ ├── db      │    │ Task management  │    │ Worktrees   │
│ ├── session │    │ Dependencies     │    │ Branches    │
│ └── worktrees    │ Status tracking  │    │ Merges      │
└─────────────┘    └──────────────────┘    └─────────────┘
```

## Source Code Structure

```
src/
├── main.rs              # CLI entry point, command routing
├── cli/mod.rs           # Clap command definitions
├── beads.rs             # beads CLI integration (bd commands)
├── worktree.rs          # Git worktree operations
├── db/                  # SQLite database (claims, symbols)
├── indexer/             # Tree-sitter symbol extraction
├── updater.rs           # Self-update functionality
└── tools/
    ├── mod.rs           # Tool exports
    ├── session.rs       # Session management for stop hooks
    ├── claim.rs         # Claim specific bead
    ├── next.rs          # Claim next ready bead
    ├── release.rs       # Release bead (merge/cleanup)
    ├── stale.rs         # Find/cleanup abandoned claims
    ├── list.rs          # List active claims
    ├── abort.rs         # Abort merge conflict
    ├── resolve.rs       # Resolve merge conflict
    ├── symbols.rs       # Symbol search
    └── context/         # Context generation
```

## Key Modules

### Session Management (`src/tools/session.rs`)

Manages `.bacchus/session.json` for stop hook integration:

```rust
pub struct Session {
    pub mode: String,           // "agent" | "orchestrator"
    pub bead_id: Option<String>, // For agent mode
    pub max_concurrent: Option<i32>, // For orchestrator mode
    pub started_at: String,
}

// Key functions:
pub fn start_session(mode, bead_id, max_concurrent) -> Result<String>
pub fn stop_session() -> Result<String>
pub fn session_status() -> Result<Value>
pub fn check_session() -> HookCheckOutput  // For stop hook
```

**Workspace root detection priority:**
1. `CLAUDE_PROJECT_DIR` env var (set by Claude Code for hooks)
2. Walk up from CWD looking for `.bacchus`, `.beads`, or `.git`

### Beads Integration (`src/beads.rs`)

Wraps the `bd` CLI for task management:

```rust
pub fn get_ready_beads() -> Result<Vec<BeadInfo>>      // bd ready --json
pub fn get_in_progress_beads() -> Result<Vec<BeadInfo>> // bd list --status=in_progress
pub fn get_bead(bead_id) -> Result<BeadInfo>           // bd show <id> --json
pub fn update_bead_status(bead_id, status) -> Result<()>
pub fn is_bead_ready(bead_id) -> Result<bool>
```

### Worktree Operations (`src/worktree.rs`)

Git worktree management:

```rust
pub fn create_worktree(bead_id, workspace_root) -> Result<WorktreeInfo>
pub fn remove_worktree(worktree_path, workspace_root) -> Result<()>
pub fn merge_worktree(bead_id, workspace_root) -> Result<MergeResult>
```

## Plugin Structure

```
plugin/
├── .claude-plugin/config.json  # Plugin manifest
├── hooks/
│   ├── hooks.json              # Hook registration
│   ├── stop-router.sh          # Delegates to bacchus session check
│   └── stop-prompt.md          # Alternative prompt-based hook
├── commands/
│   ├── agent.md                # /bacchus-agent command
│   ├── orchestrate.md          # /bacchus-orchestrate command
│   └── cancel.md               # /bacchus-cancel command
├── skills/
│   ├── planner.md              # Task breakdown skill
│   └── context.md              # Context generation skill
└── scripts/
    └── session.sh              # Shell helper (fallback)
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
        │   └─► bd show <bead_id>
        │       ├─► closed → approve (clear session)
        │       └─► not closed → block
        │
        └─► Orchestrator mode:
            ├─► ready beads + capacity → block (spawn agents)
            ├─► active claims → block (wait)
            ├─► in_progress without claims → block (orphaned)
            └─► all done/blocked → approve (clear session)
```

## Development

### Build

```bash
cargo build           # Debug build
cargo build --release # Release build
cargo test            # Run tests
```

### Local Testing

```bash
# Test session commands
./target/debug/bacchus session start agent --bead-id "TEST-123"
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

### Claims Table (bacchus.db)

```sql
CREATE TABLE claims (
    bead_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    claimed_at INTEGER NOT NULL  -- Unix timestamp in ms
);
```

### Symbols Table (bacchus.db)

```sql
CREATE TABLE symbols (
    file TEXT NOT NULL,
    fq_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    span_start_line INTEGER,
    span_end_line INTEGER,
    line_count INTEGER,
    hash TEXT,
    docstring TEXT,
    language TEXT,
    PRIMARY KEY (file, fq_name)
);
```

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

## Dependencies

- **Required**: `bd` (beads CLI), `git`
- **Build**: Rust toolchain, tree-sitter
- **Runtime**: SQLite (bundled via rusqlite)
