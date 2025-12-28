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
  - `bead_id`: string (agent mode only) - the assigned bead
  - `max_concurrent`: number (orchestrator mode only) - max parallel agents
  - `started_at`: ISO timestamp
- `path`: string - path to session file

## Evaluation Steps

### If no active session (active = false)
Approve exit - this is not a bacchus-managed session.

### If session.mode = "agent"

1. Run: `bd show <bead_id> --json` (using session.bead_id from session status)
2. Check the `status` field:
   - If `status` is "closed" → APPROVE exit
   - If `status` is anything else → BLOCK exit

Consider blocking reasons:
- Task not complete
- Tests failing
- Acceptance criteria not met
- Work uncommitted

### If session.mode = "orchestrator"

1. Run: `bd status --json` to get project stats
2. Run: `bd ready --json` to get ready beads
3. Run: `bacchus list` to get active agents

Decision matrix (use session.max_concurrent from session status):
- Ready beads exist AND active agents < max_concurrent → BLOCK (spawn more agents)
- In-progress beads exist → BLOCK (wait for completion)
- Only blocked beads remain → APPROVE (needs manual intervention)
- All beads closed → APPROVE (work complete)

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
  "reason": "Bead beads-abc123 status is 'in_progress'. Continue working until complete, then run 'bd close beads-abc123'."
}
```

**Orchestrator - more work available:**
```json
{
  "decision": "block",
  "reason": "3 ready beads available. Spawn agents with 'bacchus claim <bead_id> worker-N' for: beads-abc, beads-def, beads-ghi"
}
```

**All complete:**
```json
{
  "decision": "approve",
  "reason": "All 5 beads closed. Work complete."
}
```
