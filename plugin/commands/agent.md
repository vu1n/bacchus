---
name: bacchus-agent
description: Start a persistent agent on a bead. Agent keeps working until bead is closed.
arguments:
  - name: bead_id
    description: The bead ID to work on (e.g., BACH-abc123)
    required: true
---

# Bacchus Agent Mode

You are now operating in **Bacchus Agent Mode**. You will work on bead `{{bead_id}}` until it is complete.

## Start Session

Run these commands to activate the stop hook and claim the bead:

```bash
# Start session (activates stop hook)
bacchus session start agent --bead-id "{{bead_id}}"

# Claim the bead and create worktree
bacchus claim "{{bead_id}}" agent-$$
```

The stop hook will now prevent you from stopping until `{{bead_id}}` is closed.

## Get Task Details

```bash
bd show {{bead_id}}
```

## Work in the Worktree

After claiming, work in the isolated worktree at `.bacchus/worktrees/{{bead_id}}/`.

> **Warning**: Do NOT `cd` into the worktree. Use `git -C` or absolute paths instead. The worktree is ephemeral and gets deleted on release - if your cwd points there, the shell breaks.

```bash
# Use -C flag for git operations
git -C .bacchus/worktrees/{{bead_id}} status
git -C .bacchus/worktrees/{{bead_id}} add .
git -C .bacchus/worktrees/{{bead_id}} commit -m "message"

# For other commands, use absolute paths
cat .bacchus/worktrees/{{bead_id}}/src/file.rs
```

## Your Mission

1. **Understand the task** from the bead details
2. **Implement the solution** in the worktree (use `-C` flag)
3. **Commit your changes** as you go
4. **Close the bead** when complete: `bd close {{bead_id}}`
5. **Release the worktree**: `bacchus release {{bead_id}} --status done`

The stop hook will auto-clear the session when the bead is closed.

## Completion Criteria

Before closing the bead:
- [ ] All acceptance criteria met
- [ ] Code compiles/builds without errors
- [ ] Tests pass (if applicable)
- [ ] Changes committed

## Commands Reference

```bash
# Check bead status
bd show {{bead_id}}

# Add progress notes
bd comment {{bead_id}} "Implemented X, working on Y"

# Mark as blocked if stuck
bd update {{bead_id}} --status blocked
bd comment {{bead_id}} "Blocked by: ..."

# Complete the work
bd close {{bead_id}}
bacchus release {{bead_id}} --status done
```

## Force Exit

If you need to exit without completing:

```bash
bacchus session stop
```

---

Now start by checking the task details:

```bash
bd show {{bead_id}}
```
