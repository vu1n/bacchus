# CLAUDE.md - Bacchus

Workspace-based coordination CLI for multi-agent work. Uses jj workspaces + SQLite.

## Quick Reference

### Agent Workflow

```
1. Orchestrator spawns agent with archetype prompt
2. bacchus claim <task-id> --agent-id <agent-id>
3. Work in .bacchus/workspaces/<task-id>/ (use jj -R, NEVER cd)
4. jj -R .bacchus/workspaces/<task-id>/ rebase -d main
5. Review loop until green (rebase → /tighten → bacchus review → fix → repeat)
6. /ship-review (final pre-release gate)
7. bacchus release <task-id> --status done
```

### Key Commands

| Command | Purpose |
|---------|---------|
| `bacchus task list --ready` | Show claimable tasks |
| `bacchus claim <id> --agent-id <agent>` | Claim task, create workspace |
| `bacchus next --agent-id <agent>` | Auto-claim next ready task |
| `bacchus release <id> --status done` | Mark ready for merge |
| `bacchus session start agent --task-id <id>` | Enable stop hook |
| `bacchus init --runner <claude\|codex>` | Bootstrap; pick the worker runner |
| `bacchus worker-prompt <agent> <id>` | Emit worker protocol as text (codex runner) |
| `jj -R .bacchus/workspaces/<id> status` | Check workspace |
| `jj -R .bacchus/workspaces/<id> describe -m "msg"` | Commit message |

### Critical: Never cd into workspace

Workspaces are deleted on release. Always use `jj -R <path>` instead.

## Design Rationale

### Why SQLite (not YAML)?

- **Atomic claims**: `UPDATE ... WHERE status='open'` prevents races
- **Footprint collision**: Subquery checks in-progress task overlap
- **Crash recovery**: Transactions survive process death
- **Dependency queries**: Recursive CTEs for ready calculation

YAML cannot provide atomic read-modify-write or embedded subqueries.

### Why type vs archetype separation?

Orthogonal concerns:
- **task_type** = PM workflow (what kind of work)
- **archetype** = Agent expertise (what skills needed)

A `feature` task could need `frontend` OR `backend` archetype. Planner assigns archetype explicitly.

## Key Structures

### task_type (PM workflow)

| Type | When to use |
|------|-------------|
| `bug_fix` | Fixing defects |
| `feature` | New functionality |
| `refactor` | Restructure, preserve behavior |
| `test` | Add/improve tests |
| `docs` | Documentation |
| `infra` | CI/CD, deployment |
| `generic` | Default |

### archetype (Agent specialization)

| Archetype | Focus |
|-----------|-------|
| `design` | Visual identity, design system, tokens, typography, high-craft UI |
| `frontend` | UI, components, CSS, a11y |
| `backend` | APIs, auth, validation |
| `data` | Pipelines, SQL, schemas |
| `test` | Coverage, fixtures, e2e |
| `infra` | CI/CD, containers, cloud |
| `review` | Quality, patterns |
| `security` | Vulnerabilities, OWASP |
| `docs` | Documentation, READMEs, API docs, guides |
| `generic` | Default |

Source of truth: `archetypes.yaml`. Project override: `.bacchus/archetypes.yaml`.

### Task Status Flow

```
draft → open → in_progress → ready_for_release → releasing → closed
                    ↓                                  ↓
                 failed                        needs_resolution
```

## Token-Saving Handles

Use `--handle` flag to return compact pointers instead of full data:

```bash
bacchus symbols --search "auth" --handle   # Returns $sym1
bacchus handle expand $sym1 --limit 5      # Get actual data
bacchus handle filter $sym1 --kind fn      # Returns $sym2
bacchus handle clear                       # Cleanup
```

Handles are session-scoped and auto-cleared on `session stop`.

## Source Structure

```
src/
├── main.rs           # CLI entry, routing
├── handles.rs        # Token-saving handle system
├── tasks.rs          # SQLite task ops, YAML import
├── workspace.rs      # jj workspace ops
├── db/               # Schema, migrations
└── tools/
    ├── claim.rs      # Claim task
    ├── release.rs    # Release task
    ├── session.rs    # Stop hook integration
    ├── symbols.rs    # Symbol search (supports handles)
    └── archetypes.rs # Archetype selection
```

## Development

```bash
cargo build                          # Debug
cargo build --release                # Release
cargo test -- --test-threads=1       # Tests (sequential, shared DB)
cp target/release/bacchus ~/.local/bin/
```

## Release

```bash
# 1. Bump version in Cargo.toml
# 2. Commit
git tag -a v0.X.0 -m "v0.X.0: Description"
git push origin v0.X.0
# GitHub Actions builds + releases
```

## Database Schema

```sql
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    epic_id TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'draft',
    task_type TEXT NOT NULL DEFAULT 'generic',
    archetype TEXT NOT NULL DEFAULT 'generic',
    claimed_by TEXT,
    claimed_at INTEGER,
    ready_commit_id TEXT,
    CHECK (task_type IN ('bug_fix','feature','refactor','test','docs','infra','generic')),
    CHECK (archetype IN ('design','frontend','backend','data','test','infra','review','security','docs','generic'))
);

-- Token-saving handle system
CREATE TABLE handles (
    handle TEXT PRIMARY KEY,        -- $sym1, $ctx2, etc.
    handle_type TEXT NOT NULL,      -- symbols, context, messages
    count INTEGER NOT NULL,
    query TEXT,                     -- Original query
    session_id TEXT                 -- For session cleanup
);

CREATE TABLE handle_data (
    handle TEXT NOT NULL,
    idx INTEGER NOT NULL,
    data TEXT NOT NULL,             -- JSON serialized
    PRIMARY KEY (handle, idx)
);
```

## Configuration

Project config lives in `.bacchus/config.yaml`. Generated by `bacchus init`.

```yaml
quality:
  check: "cargo check --quiet"
  test: "cargo test --quiet"
  lint: "cargo clippy --quiet -- -D warnings"

worker:
  runner: "claude"       # "claude" (default) or "codex"; selects the default cmd
  cmd: "claude --dangerously-skip-permissions -p '/bacchus-worker $BACCHUS_AGENT_ID $BACCHUS_TASK_ID'"
  # cmd overrides runner. Codex runner default (set via `bacchus init --runner codex`):
  #   cmd: "codex exec --dangerously-bypass-approvals-and-sandbox \"$(bacchus worker-prompt $BACCHUS_AGENT_ID $BACCHUS_TASK_ID)\""
  # auto_spawn: true     # Enable auto-spawn (default: true)
  # retry_backoff_ms: 60000
  # max_retries: 3
  # stale_grace_ms: 60000
  # max_runtime_ms: null
  # kill_stale: false

memory:                  # Shared memory via kypp (opt-in; requires `kypp` on PATH)
  # enabled: true
  # project: "my-project"  # KYPP_PROJECT; defaults to the project directory name
```

### Shared Memory (kypp)

When `memory.enabled`, bacchus scopes each worker's environment
(`KYPP_PROJECT`, `KYPP_REPO_ROOT`, `BACCHUS_MEMORY=1`) so the worker prompt's
`kypp briefing` / `recall` / `remember` steps bind the right store. Integration
is **agent-driven and fail-open**: if `kypp` is not installed the steps no-op.
Code grounding (`KYPP_REPO_ROOT`) points at the canonical project tree, not the
ephemeral per-task workspace. (Auto-capture/distill of session logs is Phase 2 —
it depends on `§0` session logs that arrive with the pillbox sandbox.)

## Environment

Env vars override `.bacchus/config.yaml` values for CI/scripting.

| Variable | Purpose |
|----------|---------|
| `CLAUDE_PROJECT_DIR` | Workspace root (set by Claude Code) |
| `BACCHUS_DB_PATH` | Override DB location |
| `BACCHUS_WORKER_CMD` | Override `worker.cmd` from config.yaml |
| `BACCHUS_MEMORY` | Set to `1` by bacchus when `memory.enabled` — signals workers to use kypp |
| `KYPP_PROJECT` | kypp project binding, set per-worker from `memory.project` |
| `KYPP_REPO_ROOT` | Repo root kypp grounds code refs against (the project tree) |

## Error Handling

- Stop hooks fail-open (never trap user)
- Claims validate readiness unless `--force`
- Conflicts return structured error for orchestrator

## jj Reference

| Action | Command |
|--------|---------|
| Status | `jj -R <ws> status` |
| Diff | `jj -R <ws> diff` |
| Describe | `jj -R <ws> describe -m "msg"` |
| Log | `jj -R <ws> log` |
| Resolve | `jj -R <ws> resolve` |

Why jj? Non-blocking conflicts, auto-snapshot, simpler rebase, lightweight workspaces.
