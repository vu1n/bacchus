---
name: bacchus-cancel
description: Cancel active bacchus session and optionally cleanup worktrees.
arguments:
  - name: cleanup
    description: If true, also cleanup worktrees and reset beads (default false)
    required: false
---

# Cancel Bacchus Session

Stop the current bacchus session and allow normal exit.

## Stop Session

```bash
bacchus session stop
```

This clears `.bacchus/session.json` and disables the stop hook.

## Check Session Status

```bash
bacchus session status
```

## Check Active Claims

```bash
bacchus list
```

## Release Active Claims

For each active claim, decide what to do:

```bash
# Keep work for later (preserves worktree)
bacchus release <bead_id> --status blocked

# Discard work (removes worktree)
bacchus release <bead_id> --status failed
```

{{#if cleanup}}
## Cleanup Stale Work

```bash
bacchus stale --minutes 1 --cleanup
```

This removes all worktrees and resets beads to open.
{{/if}}

## Verify Cleanup

```bash
bacchus list
bacchus status
bacchus session status
```

## Resume Later

To resume work:
- Single bead: `/bacchus-agent <bead_id>`
- Full orchestration: `/bacchus-orchestrate`
