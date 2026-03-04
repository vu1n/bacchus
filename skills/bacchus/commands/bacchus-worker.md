# Bacchus Worker Agent

You are a worker agent coordinated by `bacchus`. Follow this protocol exactly.

## Arguments

- `$ARGUMENTS` contains: `<AGENT_ID> <TASK_ID>` (e.g., `worker-1 AUTH-001`)
- If only AGENT_ID is given, use `bacchus next` to find work.

Parse arguments:
```
AGENT_ID = first word of $ARGUMENTS
TASK_ID  = second word of $ARGUMENTS (optional)
```

## Workflow

### 1. Start Session & Claim Work

```bash
# If TASK_ID was provided:
bacchus session start agent --agent-id <AGENT_ID> --task-id <TASK_ID>
bacchus claim <TASK_ID> --agent-id <AGENT_ID>

# If no TASK_ID — find the next ready task:
bacchus next --agent-id <AGENT_ID>
```

### 2. Understand the Task

```bash
bacchus task show <TASK_ID>
bacchus context --task-id <TASK_ID>
```

Read the output carefully. Context includes:
- Task description, type, archetype
- Footprint (files/symbols you own)
- Active collisions with other agents
- Risk hints

### 3. Load Your Archetype

The task's `archetype` field determines your domain expertise. Load it:

```bash
bacchus archetype prompt <ARCHETYPE>
```

Read the archetype prompt and adopt its approach for the rest of this task.
The archetype tells you *how* to think about the work (e.g., frontend: check for design system first; backend: match existing error shapes; test: read implementation before writing tests).

### 4. Do the Work

All work happens in `.bacchus/workspaces/<TASK_ID>/`.

**CRITICAL: Never `cd` into the workspace.** It is deleted on release.
Always use `jj -R .bacchus/workspaces/<TASK_ID>/` for VCS commands.
Edit files using full paths like `.bacchus/workspaces/<TASK_ID>/src/foo.rs`.

**CRITICAL: Do NOT run package manager install commands** (`bun install`, `npm install`,
`pnpm install`, `yarn install`, etc.). The orchestrator handles dependency installation
after merging your changes. You may add dependencies to `package.json` — just don't run install.

#### Test-First Tasks

If the task description contains a `## Test-First` section, follow its instructions:
write tests first, then implement to pass them. Do not skip the test step.

The pre-release quality gate will run the project's test suite. Your release will be
blocked if tests fail.

jj auto-snapshots — no explicit commit step needed.

```bash
jj -R .bacchus/workspaces/<TASK_ID>/ status    # check status
jj -R .bacchus/workspaces/<TASK_ID>/ diff      # view changes
jj -R .bacchus/workspaces/<TASK_ID>/ log       # view history
```

### 5. Stay Within Your Footprint

Your task declares what you're allowed to touch:
- `modifies` — existing files/symbols to change
- `creates` — new files to add

If you need code outside your footprint, release as `blocked` and message the orchestrator.

### 6. Heartbeat During Long Work

```bash
bacchus heartbeat <TASK_ID> --agent-id <AGENT_ID>
```

Send periodically to avoid being marked stale (default timeout: 15 min).

### 7. Rebase and Review Before Release

Before releasing, rebase onto current main so the quality gate and /simplify run against up-to-date code:

```bash
# a. Rebase your workspace onto main
jj -R .bacchus/workspaces/<TASK_ID>/ rebase -d main

# b. If conflicts appear, resolve them:
jj -R .bacchus/workspaces/<TASK_ID>/ resolve
# Then re-run build/tests to verify resolution

# c. Run /simplify to review your changes for quality
#    (reuse, efficiency, consistency with codebase patterns)
#    Fix any findings before proceeding

# d. Run the quality gate
bacchus review <TASK_ID>
```

Do NOT release as `done` if:
- Rebase conflicts remain unresolved
- `/simplify` found unaddressed issues
- `bacchus review` fails

### 8. Describe and Release

```bash
# Describe your changes
jj -R .bacchus/workspaces/<TASK_ID>/ describe -m "concise summary of changes"

# Release
bacchus release <TASK_ID> --status done      # success
bacchus release <TASK_ID> --status blocked   # need help / out-of-footprint
bacchus release <TASK_ID> --status failed    # discard and reset
```

### 9. Handle Merge Conflicts (if notified)

```bash
bacchus resolve <TASK_ID>    # after fixing conflicts
bacchus abort <TASK_ID>      # abandon resolution, keep working
```

## Communication

```bash
bacchus message send --from <AGENT_ID> --to orchestrator --body "message"
bacchus message list --agent <AGENT_ID>
bacchus message ack <MESSAGE_ID>
```

## Token-Saving Handles

```bash
bacchus symbols --search "auth" --handle     # → $sym1
bacchus handle expand $sym1 --limit 5        # retrieve data
bacchus handle filter $sym1 --kind fn        # → $sym2
bacchus handle clear                         # cleanup
```

## Rules

1. Never cd into a workspace directory
2. Always use `jj -R` for workspace VCS operations
3. Stay within your declared footprint
4. Send heartbeats during long work
5. Describe changes before releasing
6. Don't mark `done` if tests fail
7. Check context for collision warnings before starting
8. Never run `bun install` / `npm install` / `pnpm install` — orchestrator handles this
