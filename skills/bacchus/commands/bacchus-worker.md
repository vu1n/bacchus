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

### 3. Archetype Context

Your archetype prompt was injected at spawn time — it's already part of your system context.
If you need to re-read it (e.g., after context compaction), run:

```bash
bacchus archetype prompt <ARCHETYPE>
```

The archetype tells you *how* to think about the work (e.g., frontend: check for design system
first; backend: match existing error shapes; test: read implementation before writing tests).
Follow its domain-specific guidance throughout the task.

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

### 6. Heartbeat During Work

Send heartbeats to avoid being marked stale (timeout: 15 min).
**Trigger:** Send a heartbeat before every `jj` command and after completing each logical step
(e.g., after writing a file, after running tests). This keeps you alive without guessing intervals.

```bash
curl -s --max-time 1 -X POST -H 'Content-Type: application/json' \
  -d "{\"task_id\":\"$BACCHUS_TASK_ID\",\"agent_id\":\"$BACCHUS_AGENT_ID\",\"activity\":\"editing\"}" \
  "http://127.0.0.1:${BACCHUS_EVENT_PORT}/heartbeat" >/dev/null 2>&1
```

If `$BACCHUS_EVENT_PORT` is not set, fall back to:
```bash
bacchus activity <TASK_ID> <AGENT_ID> "editing"
```

### 7. Review Loop (repeat until green, max 5 attempts)

Before releasing, enter this loop:

1. Rebase onto main: `jj -R .bacchus/workspaces/<TASK_ID>/ rebase -d main`
2. Resolve any conflicts: `jj -R .bacchus/workspaces/<TASK_ID>/ resolve`
3. Run `/simplify` — fix all findings
4. Run `bacchus review <TASK_ID>`
5. **If review fails with code issues**: fix the failing checks, then go back to step 1
6. **If review passes**: proceed to Release (step 8)

**Circuit breaker:** If the review loop fails 5 times, release as `blocked` with the error
details. If every failure is the **same infrastructure error** (not code quality), release as
`blocked` after 3 attempts — don't wait for 5.

Do NOT release as `done` until the review loop passes.
Do NOT stop or exit until you have released your task.

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
