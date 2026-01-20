---
name: bacchus-agent
description: Start a persistent agent on a task. Agent keeps working until task is closed.
arguments:
  - name: task_id
    description: The task ID to work on (e.g., AUTH-001)
    required: true
---

# Bacchus Agent Mode

You are now operating in **Bacchus Agent Mode**. You will work on task `{{task_id}}` until it is complete.

## Start Session

Run these commands to activate the stop hook and claim the task:

```bash
# Start session (activates stop hook)
bacchus session start agent --task-id "{{task_id}}"

# Claim the task and create worktree
bacchus claim "{{task_id}}" agent-$$
```

The stop hook will now prevent you from stopping until `{{task_id}}` is closed.

## Get Task Context

Get rich, type-aware context for your task:

```bash
bacchus context --task-id {{task_id}}
```

This provides:
- Task info (title, description, type, priority)
- Dependencies (what blocks this, what this unblocks)
- Footprint (files/symbols to modify/create)
- Type-specific guidance

## Work in the Worktree

After claiming, work in the isolated worktree at `.bacchus/worktrees/{{task_id}}/`.

> **Warning**: Do NOT `cd` into the worktree. Use `git -C` or absolute paths instead. The worktree is ephemeral and gets deleted on release - if your cwd points there, the shell breaks.

```bash
# Use -C flag for git operations
git -C .bacchus/worktrees/{{task_id}} status
git -C .bacchus/worktrees/{{task_id}} add .
git -C .bacchus/worktrees/{{task_id}} commit -m "message"

# For other commands, use absolute paths
cat .bacchus/worktrees/{{task_id}}/src/file.rs
```

## Your Mission

1. **Understand the task** from the task details
2. **Implement the solution** in the worktree (use `-C` flag)
3. **Commit your changes** as you go
4. **Release the task**: `bacchus release {{task_id}} --status done`

The release command will merge the worktree, close the task, and clear the session.

## Completion Criteria

Before releasing the task:
- [ ] All acceptance criteria met
- [ ] Code compiles/builds without errors
- [ ] Tests pass (if applicable)
- [ ] Changes committed

## Commands Reference

```bash
# Check task status
bacchus task show {{task_id}}

# List all tasks
bacchus task list

# Check ready tasks
bacchus task list --ready

# Complete the work
bacchus release {{task_id}} --status done

# If blocked, release without merging
bacchus release {{task_id}} --status blocked
```

## Force Exit

If you need to exit without completing:

```bash
bacchus session stop
```

---

Now start by getting your task context:

```bash
bacchus context --task-id {{task_id}}
```
