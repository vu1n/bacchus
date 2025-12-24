---
name: bacchus
description: Multi-agent coordination CLI for codebases. Use when orchestrating parallel agents, claiming tasks, detecting symbol conflicts, or notifying stakeholders of breaking changes. Invoke when user mentions coordination, parallel agents, multiple agents, task claiming, or conflict detection.
---

# Bacchus - Worktree-Based Agent Coordination

Lightweight coordination for parallel agent work. Uses git worktrees for isolation and integrates with beads for task management.

## Installation

```bash
curl -fsSL https://raw.githubusercontent.com/vu1n/bacchus/main/scripts/install.sh | bash
```

## Core Workflow

```
next → work in worktree → release
```

### 1. Get Work

```bash
bacchus next agent-1
```

This command:
- Queries beads DB for ready tasks (open, no blockers)
- Creates isolated worktree at `.bacchus/worktrees/{bead_id}/`
- Claims the bead, updates status to `in_progress`

Output:
```json
{
  "success": true,
  "bead_id": "PROJ-42",
  "title": "Implement user auth",
  "worktree_path": ".bacchus/worktrees/PROJ-42",
  "branch": "bacchus/PROJ-42"
}
```

### 2. Do Work

Work in the worktree directory. All changes are isolated on branch `bacchus/{bead_id}`.

```bash
cd .bacchus/worktrees/PROJ-42
# make changes, commit normally
git add . && git commit -m "Implement auth"
```

### 3. Release

```bash
# Success - merge to main, cleanup worktree, close bead
bacchus release PROJ-42 --status done

# Blocked - keep worktree for later, mark bead blocked
bacchus release PROJ-42 --status blocked

# Failed - discard changes, reset bead to open
bacchus release PROJ-42 --status failed
```

## Stale Detection

Find and cleanup abandoned claims:

```bash
# List stale claims (>30 min old)
bacchus stale --minutes 30

# Auto-cleanup: remove worktrees, reset beads to open
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
      "bead_id": "PROJ-42",
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

## Relationship to Beads

```
beads    → What work needs to be done (issues, deps, status)
bacchus  → Who's doing what right now (claims, worktrees)
```

Bacchus reads from beads to find ready work and updates bead status on claim/release. Use beads for any workflow. Add bacchus when multiple agents work in parallel to ensure isolation via worktrees.

## Directory Structure

```
project/
├── .bacchus/
│   ├── bacchus.db          # Claims database
│   └── worktrees/
│       ├── PROJ-42/        # Isolated worktree for PROJ-42
│       └── PROJ-43/        # Another agent's worktree
└── .beads/
    └── beads.db            # Task/issue database
```
