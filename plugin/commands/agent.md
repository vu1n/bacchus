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

# Claim the task and create jj workspace
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

## Work in the Workspace

After claiming, work in the isolated jj workspace at `.bacchus/workspaces/{{task_id}}/`.

> **Warning**: Do NOT `cd` into the workspace. Use `jj -R` or absolute paths instead. The workspace is ephemeral and gets deleted on release - if your cwd points there, the shell breaks.

```bash
# Use -R flag for jj operations (changes are auto-snapshotted)
jj -R .bacchus/workspaces/{{task_id}} status
jj -R .bacchus/workspaces/{{task_id}} describe -m "Implement feature"
jj -R .bacchus/workspaces/{{task_id}} diff

# For reading files, use absolute paths
cat .bacchus/workspaces/{{task_id}}/src/file.rs
```

## Your Mission

1. **Understand the task** from the task details
2. **Implement the solution** in the workspace (use `-R` flag)
3. **Describe your change**: `jj -R <workspace> describe -m "message"`
4. **Release the task**: `bacchus release {{task_id}} --status done`

The release marks the task ready for the orchestrator to merge. The orchestrator will rebase your commit onto main and close the task.

## Completion Criteria

Before releasing the task:
- [ ] All acceptance criteria met
- [ ] Code compiles/builds without errors
- [ ] Tests pass (if applicable)
- [ ] Change described with `jj describe`

## Commands Reference

```bash
# Check task status
bacchus task show {{task_id}}

# List all tasks
bacchus task list

# Check ready tasks
bacchus task list --ready

# Complete the work (marks ready for orchestrator)
bacchus release {{task_id}} --status done

# If blocked, release without completing
bacchus release {{task_id}} --status blocked
```

## Conflict Resolution

If the orchestrator reports conflicts during merge:
1. Your task will be marked `needs_resolution`
2. Resolve conflicts in your workspace: `jj -R .bacchus/workspaces/{{task_id}} resolve`
3. Mark resolved: `bacchus resolve {{task_id}}`

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
