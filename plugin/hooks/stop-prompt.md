# Bacchus Stop Hook (Prompt-Based)

You are evaluating whether Claude should stop working in a Bacchus-managed session.

## Context

Session input: $ARGUMENTS

First, check for an active session:
```bash
bacchus session status
```

This returns JSON with:
- `active`: boolean - whether a session is active
- `session`: object (when active) containing:
  - `mode`: "agent" | "orchestrator" - session type
  - `task_id`: string (agent mode only) - the assigned task
  - `max_concurrent`: number (orchestrator mode only) - max parallel agents
  - `started_at`: ISO timestamp
- `path`: string - path to session file

## Evaluation Steps

### If no active session (active = false)
Approve exit - this is not a bacchus-managed session.

### If session.mode = "agent"

1. Run: `bacchus task show <task_id>` (using session.task_id from session status)
2. Check the `status` field:
   - If `status` is "closed" → APPROVE exit
   - If `status` is anything else → BLOCK exit

Consider blocking reasons:
- Task not complete
- Tests failing
- Acceptance criteria not met
- Work uncommitted

### If session.mode = "orchestrator"

1. Run: `bacchus task list` to get project stats
2. Run: `bacchus task list --ready` to get ready tasks
3. Run: `bacchus list` to get active agents

Decision matrix (use session.max_concurrent from session status):
- Ready tasks exist AND active agents < max_concurrent → BLOCK (spawn more agents)
- In-progress tasks exist → BLOCK (wait for completion)
- Only blocked tasks remain → APPROVE (needs manual intervention)
- All tasks closed → APPROVE (work complete)

## Response Format

Respond with JSON only:

```json
{
  "decision": "approve" | "block",
  "reason": "Explanation for the decision"
}
```

### Example Responses

**Agent - task incomplete:**
```json
{
  "decision": "block",
  "reason": "Task TASK-123 status is 'in_progress'. Continue working until complete, then run 'bacchus release TASK-123 --status done'."
}
```

**Orchestrator - more work available:**
```json
{
  "decision": "block",
  "reason": "3 ready tasks available. Spawn agents with 'bacchus claim <task_id> worker-N' for: TASK-abc, TASK-def, TASK-ghi"
}
```

**All complete:**
```json
{
  "decision": "approve",
  "reason": "All 5 tasks closed. Work complete."
}
```
