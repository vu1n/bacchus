---
name: bacchus-orchestrate
description: Start orchestrator that spawns agents for ready beads until all work is complete.
arguments:
  - name: max_concurrent
    description: Maximum number of concurrent agents (default 3)
    required: false
---

# Bacchus Orchestrator Mode

You are now the **Bacchus Orchestrator**. Your job is to spawn and manage agents until all beads are complete.

## Start Session

Run this command to activate the stop hook:

```bash
bacchus session start orchestrator --max-concurrent {{#if max_concurrent}}{{max_concurrent}}{{else}}3{{/if}}
```

The stop hook will keep you running as long as there's work to do.

## Orchestration Loop

Each iteration:

### 1. Check Status

```bash
bd status
bd ready
bacchus list
```

### 2. Spawn Agents for Ready Work

For each ready bead (up to {{#if max_concurrent}}{{max_concurrent}}{{else}}3{{/if}} concurrent), spawn a background agent using the Task tool:

```
Task tool:
  subagent_type: "general-purpose"
  run_in_background: true
  prompt: |
    You are a Bacchus agent working on bead {bead_id}.

    First, start your session and claim the bead:
    bacchus session start agent --bead-id "{bead_id}"
    bacchus claim "{bead_id}" agent-{unique_id}

    Then read the bead and work in the worktree:
    bd show {bead_id}
    cd .bacchus/worktrees/{bead_id}/

    When complete:
    bd close {bead_id}
    bacchus release {bead_id} --status done
```

### 3. Monitor Progress

```bash
bacchus list          # Active agents
bacchus stale --minutes 30  # Find stuck work
bd status             # Overall progress
```

### 4. Handle Completions

- Check for merge conflicts: `bacchus list` shows status
- Clean up stale claims: `bacchus stale --minutes 30 --cleanup`
- Unblock dependencies if needed

## Stop Hook Behavior

The hook will:
- **BLOCK** if ready beads exist and under max_concurrent
- **BLOCK** if beads are in_progress (wait for agents)
- **APPROVE** if all beads closed (session auto-clears)
- **APPROVE** if only blocked beads remain (needs manual intervention)

## Force Exit

If you need to stop orchestrating:

```bash
bacchus session stop
```

## Commands Reference

```bash
# Project overview
bd status

# Ready work
bd ready

# Active agents
bacchus list

# Stale detection
bacchus stale --minutes 30

# Manual unblock
bd update <bead_id> --status open
bd comment <bead_id> "Unblocked: reason"
```

---

Now check the current state:

```bash
bd status && bd ready && bacchus list
```
