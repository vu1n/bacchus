---
name: bacchus-context
description: Generate context for the current bacchus session - either global orchestrator view or focused agent view.
---

# Bacchus Context

Generate a focused context summary for the current working state.

## Usage

```bash
bacchus context
```

This command auto-detects mode:
- **Repo root**: Global view (all claims, ready work, project stats)
- **In worktree**: Task view (specific bead objectives, related symbols)

## Global Context (Orchestrator)

When run from repo root, shows:
- Active claims and their age
- Ready beads waiting for agents
- Blocked beads needing intervention
- Project health statistics

## Task Context (Agent)

When run from a worktree, shows:
- Bead details (title, description, acceptance criteria)
- Dependencies (what this unblocks)
- Related symbols in the codebase
- Suggested starting points

## Example Output

**Global Mode**:
```markdown
# Bacchus Status

## Active Work (2 agents)
- beads-abc123: "Add user auth" (agent-1, 15 min)
- beads-def456: "Write tests" (agent-2, 8 min)

## Ready Work (1 bead)
- beads-ghi789: "Update docs" (P2, no blockers)

## Blocked (1 bead)
- beads-jkl012: "Deploy" (blocked by beads-abc123)
```

**Task Mode**:
```markdown
# Task: beads-abc123

## Objective
Add user authentication with JWT tokens

## Acceptance Criteria
- [ ] Login endpoint returns JWT
- [ ] Middleware validates tokens
- [ ] Tests cover happy path and errors

## Related Symbols
- `src/auth/`: Authentication module
- `UserService.authenticate()`: Existing stub

## Unblocks
- beads-jkl012: "Deploy to production"
```

## When to Use

- **Start of orchestrator session**: Understand project state
- **Start of agent session**: Understand specific task
- **After extended work**: Re-orient and check progress
- **Before closing**: Verify all work is accounted for
