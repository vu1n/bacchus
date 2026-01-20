---
name: bacchus-context
description: Generate context for the current bacchus session - either global orchestrator view or focused agent view.
---

# Bacchus Context

Generate a focused context summary for the current working state.

## Usage

```bash
# Global context (orchestrator view)
bacchus context

# Task-specific context (agent view)
bacchus context --task-id <TASK_ID>
```

## Global Context (Orchestrator)

When run without `--task-id`, shows:
- Active claims and their age
- Ready tasks waiting for agents
- Blocked tasks needing intervention
- Project health statistics

## Task Context (Agent)

When run with `--task-id`, shows:
- Task details (title, description, type, priority)
- Dependencies (what blocks this, what this unblocks)
- Footprint (files/symbols to modify/create)
- Type-specific guidance (bug fix, feature, refactor, etc.)

## Example Output

**Global Mode** (`bacchus context`):
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

**Task Mode** (`bacchus context --task-id AUTH-001`):
```markdown
# Task: AUTH-001 - Add user authentication

## Overview
- **Type**: Feature
- **Priority**: 1
- **Status**: in_progress

## Description
Add JWT-based authentication to the API

## Unblocks
Completing this task will unblock:
- DEPLOY-001

## Footprint
**Modifies:**
- src/auth/handler.rs::AuthHandler

**Creates:**
- src/auth/middleware.rs

## Guidance
**Feature Guidance:**
- Understand the requirements before coding
- Write tests alongside implementation
- Consider edge cases and error handling
```

## When to Use

- **Start of orchestrator session**: Understand project state
- **Start of agent session**: Understand specific task
- **After extended work**: Re-orient and check progress
- **Before closing**: Verify all work is accounted for
