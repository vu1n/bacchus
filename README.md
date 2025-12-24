# Bacchus

Worktree-based coordination CLI for multi-agent work on codebases.

Bacchus helps AI agents coordinate when working on the same codebase by:
- **Worktree isolation** - each agent works in its own git worktree
- **Beads integration** - automatically picks ready tasks and updates status
- **Stale detection** - finds and cleans up abandoned work

## Installation

```bash
curl -fsSL https://raw.githubusercontent.com/vu1n/bacchus/main/scripts/install.sh | bash
```

This installs the binary and Claude Code skill.

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
# Get next ready task (creates worktree, claims it)
bacchus next agent-1

# Work in the isolated worktree
cd .bacchus/worktrees/TASK-42
# ... make changes, commit ...

# Release when done (merges to main, cleans up)
bacchus release TASK-42 --status done
```

## Commands

### Coordination

| Command | Description |
|---------|-------------|
| `next <agent_id>` | Get ready bead, create worktree, claim it |
| `release <bead_id> --status done\|blocked\|failed` | Finish work |
| `stale [--minutes N] [--cleanup]` | Find/cleanup abandoned claims |

### Symbols

| Command | Description |
|---------|-------------|
| `index <path>` | Index files for symbol search |
| `symbols [--pattern X] [--kind Y]` | Search for symbols |

### Info

| Command | Description |
|---------|-------------|
| `status` | Show current claims |
| `workflow` | Print protocol documentation |

## Workflow

```
next → work in worktree → release
```

### 1. Get Work

```bash
bacchus next agent-1
```

Output:
```json
{
  "success": true,
  "bead_id": "TASK-42",
  "title": "Implement auth",
  "worktree_path": ".bacchus/worktrees/TASK-42",
  "branch": "bacchus/TASK-42"
}
```

### 2. Do Work

Work in the worktree. All changes are isolated on branch `bacchus/{bead_id}`.

```bash
cd .bacchus/worktrees/TASK-42
# make changes
git add . && git commit -m "Implement auth"
```

### 3. Release

```bash
# Success - merge to main, cleanup worktree, close bead
bacchus release TASK-42 --status done

# Blocked - keep worktree, mark bead blocked
bacchus release TASK-42 --status blocked

# Failed - discard worktree, reset bead to open
bacchus release TASK-42 --status failed
```

## Stale Detection

Find and cleanup abandoned claims:

```bash
# List stale claims (>30 min old)
bacchus stale --minutes 30

# Auto-cleanup
bacchus stale --minutes 30 --cleanup
```

## Relationship to Beads

```
beads    → What work needs to be done (issues, deps, status)
bacchus  → Who's doing what right now (claims, worktrees)
```

Bacchus reads from beads to find ready work and updates bead status on claim/release.

## Directory Structure

```
project/
├── .bacchus/
│   ├── bacchus.db          # Claims database
│   └── worktrees/
│       ├── TASK-42/        # Agent 1's isolated worktree
│       └── TASK-43/        # Agent 2's isolated worktree
└── .beads/
    └── beads.db            # Task database
```

## Supported Languages

Symbol indexing supports:
- TypeScript / JavaScript
- Python
- Go
- Rust

## License

MIT
