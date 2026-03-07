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

Run ALL three commands — do not skip any:

```bash
bacchus task show <TASK_ID>
bacchus context --task-id <TASK_ID>
bacchus archetype prompt <ARCHETYPE>
```

**You MUST call `bacchus archetype prompt <ARCHETYPE>`** using the archetype from the task show output (e.g., `frontend`, `backend`, `data`). Even if the archetype prompt was injected at spawn time, explicitly call this command to confirm you have it. Read and internalize the archetype guidance — it governs how you approach the work.

From the context output, **extract and record**:
- Task description, type, archetype
- **Footprint**: the exact `modifies` and `creates` lists — these are your boundaries
- Active collisions with other agents
- Risk hints

### 3. Validate Footprint Before Starting

After reading context, **write down** the footprint lists:
- `modifies`: [list the exact files/symbols]
- `creates`: [list the exact files]

You will check every file you touch against these lists. If a file is not in either list, you **must not** touch it. If you discover you need to change something outside your footprint, **stop immediately** and release as `blocked`:

```bash
bacchus release <TASK_ID> --status blocked
bacchus message send --from <AGENT_ID> --to orchestrator --body "Need out-of-footprint access to <file/symbol>. Reason: <why>"
```

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

#### Verify Footprint Compliance

Before moving to the review loop, confirm:
1. Every modified file appears in `footprint.modifies`
2. Every new file appears in `footprint.creates`
3. No other files were touched

If you find violations, undo the out-of-footprint changes and release as `blocked` if you cannot complete the task within footprint.

### 5. Heartbeat During Work

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

### 6. Review Loop (MANDATORY — repeat until green, max 5 attempts)

Before releasing, you MUST complete this loop. Every step is required — do not skip any.

**For each iteration:**

1. **Rebase onto main:**
   ```bash
   jj -R .bacchus/workspaces/<TASK_ID>/ rebase -d main
   ```
2. **Resolve any conflicts:**
   ```bash
   jj -R .bacchus/workspaces/<TASK_ID>/ resolve
   ```
3. **Run `/simplify`** — THIS IS MANDATORY. Review all changed code for reuse, quality, and efficiency. Fix every finding before proceeding.
4. **Run `bacchus review <TASK_ID>`** — read the full output.
5. **If review fails with code issues:** fix every failing check, then **go back to step 1**.
6. **If review passes:** proceed to Release (step 7).

**You MUST run `/simplify` on every iteration.** Skipping it is a protocol violation.

**Circuit breaker:**
- If the loop fails **5 times**, release as `blocked` with error details.
- If every failure is the **same infrastructure error** (not code quality), release as `blocked` after **3 attempts**.

**Do NOT release as `done` until the review loop passes.**
**Do NOT stop or exit until you have released your task.**

### 7. Describe and Release

```bash
# Describe your changes
jj -R .bacchus/workspaces/<TASK_ID>/ describe -m "concise summary of changes"
```

Choose the appropriate release status:

| Situation | Status | When |
|-----------|--------|------|
| Review loop passed, all work complete | `done` | Normal success |
| Need out-of-footprint changes | `blocked` | Cannot complete within footprint |
| Review loop exhausted (5 failures) | `blocked` | Include error details in message |
| Infrastructure failure (3 same errors) | `blocked` | Include error details in message |
| Unrecoverable error, work should be discarded | `failed` | Workspace will be reset |

```bash
# Release with appropriate status
bacchus release <TASK_ID> --status done      # success — ONLY after review passes
bacchus release <TASK_ID> --status blocked   # need help / out-of-footprint / stuck
bacchus release <TASK_ID> --status failed    # discard and reset

# When releasing as blocked/failed, always message the orchestrator with details:
bacchus message send --from <AGENT_ID> --to orchestrator --body "<explain what happened>"
```

**Never release as `done` if:** review hasn't passed, tests fail, or you touched files outside footprint.

### 8. Handle Merge Conflicts (if notified)

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

1. **Never cd** into a workspace directory
2. Always use **`jj -R`** for workspace VCS operations
3. **Stay within your declared footprint** — verify before releasing
4. Send **heartbeats** during long work
5. **Always call `bacchus archetype prompt`** during context loading
6. **Always run `/simplify`** in the review loop — every iteration
7. **Describe changes** before releasing
8. **Don't mark `done`** if review hasn't passed or tests fail
9. Check context for **collision warnings** before starting
10. **Never run `bun install` / `npm install` / `pnpm install`** — orchestrator handles this
11. **Release as `blocked`** when you need out-of-footprint changes — don't proceed anyway
12. **Message the orchestrator** when releasing as blocked or failed with details