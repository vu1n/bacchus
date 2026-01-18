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
- **In worktree**: Task view (specific task objectives, related symbols)

## Global Context (Orchestrator)

When run from repo root, shows:
- Active claims and their age
- Ready tasks waiting for agents
- Blocked tasks needing intervention
- Project health statistics

## Task Context (Agent)

When run from a worktree, shows:
- Task details (title, description, acceptance criteria)
- Dependencies (what this unblocks)
- Related symbols in the codebase
- Suggested starting points

## Example Output

**Global Mode**:
```markdown
# Bacchus Status

## Active Work (2 agents)
- AUTH-001: "Add user auth" (agent-1, 15 min)
- AUTH-002: "Write tests" (agent-2, 8 min)

## Ready Work (1 task)
- AUTH-003: "Update docs" (P2, no blockers)

## Blocked (1 task)
- DEPLOY-001: "Deploy" (blocked by AUTH-001)
```

**Task Mode**:
```markdown
# Task: AUTH-001

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
- DEPLOY-001: "Deploy to production"
```

## When to Use

- **Start of orchestrator session**: Understand project state
- **Start of agent session**: Understand specific task
- **After extended work**: Re-orient and check progress
- **Before closing**: Verify all work is accounted for
