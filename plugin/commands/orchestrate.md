---
name: bacchus-orchestrate
description: Start orchestrator that spawns agents for ready tasks until all work is complete.
arguments:
  - name: max_concurrent
    description: Maximum number of concurrent agents (default 3)
    required: false
---

# Bacchus Orchestrator Mode

You are now the **Bacchus Orchestrator**. Your job is to spawn and manage agents until all tasks are complete.

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
bacchus task list
bacchus task list --ready
bacchus list
```

### 2. Spawn Agents for Ready Work

For each ready task (up to {{#if max_concurrent}}{{max_concurrent}}{{else}}3{{/if}} concurrent), spawn a background agent using the Task tool:

```
Task tool:
  subagent_type: "general-purpose"
  run_in_background: true
  prompt: |
    You are a Bacchus agent working on task {task_id}.

    First, start your session and claim the task:
    bacchus session start agent --task-id "{task_id}"
    bacchus claim "{task_id}" agent-{unique_id}

    Then read the task details:
    bacchus task show {task_id}

    Work in the jj workspace using -R flag (do NOT cd into it):
    jj -R .bacchus/workspaces/{task_id} status
    jj -R .bacchus/workspaces/{task_id} describe -m "Implement feature"

    When complete (marks ready for orchestrator merge):
    bacchus release {task_id} --status done
```

### 3. Monitor Progress

```bash
bacchus list          # Active agents
bacchus stale --minutes 30  # Find stuck work
bacchus task list     # Overall progress
```

### 4. Handle Releases

Tasks marked `ready_for_release` need to be merged:

```bash
# Check for tasks ready to merge
bacchus task list --status ready_for_release

# For each ready task, the orchestrator rebases and merges
# (This will be handled automatically - agents just mark ready)
```

### 5. Handle Conflicts

If merge conflicts occur:
- Task will be marked `needs_resolution`
- Agent needs to resolve: `jj -R .bacchus/workspaces/{task_id} resolve`
- Then mark resolved: `bacchus resolve {task_id}`

### 6. Cleanup

- Clean up stale claims: `bacchus stale --minutes 30 --cleanup`
- Unblock dependencies if needed

## Stop Hook Behavior

The hook will:
- **BLOCK** if ready tasks exist and under max_concurrent
- **BLOCK** if tasks are in_progress or ready_for_release (wait for agents/merge)
- **APPROVE** if all tasks closed (session auto-clears)
- **APPROVE** if only blocked tasks remain (needs manual intervention)

## Force Exit

If you need to stop orchestrating:

```bash
bacchus session stop
```

## Commands Reference

```bash
# Project overview
bacchus task list

# Ready work
bacchus task list --ready

# Active agents
bacchus list

# Stale detection
bacchus stale --minutes 30

# Check specific task
bacchus task show <task_id>
```

---

Now check the current state:

```bash
bacchus task list && bacchus task list --ready && bacchus list
```
