# CLAUDE.md - Bacchus

Workspace-based coordination CLI for multi-agent work. Uses jj workspaces + SQLite.

## Quick Reference

### Agent Workflow

```
1. Orchestrator spawns agent with archetype prompt
2. bacchus claim <task-id> <agent-id>
3. Work in .bacchus/workspaces/<task-id>/ (use jj -R, NEVER cd)
4. bacchus release <task-id> --status done
```

### Key Commands

| Command | Purpose |
|---------|---------|
| `bacchus task list --ready` | Show claimable tasks |
| `bacchus claim <id> <agent>` | Claim task, create workspace |
| `bacchus release <id> --status done` | Mark ready for merge |
| `bacchus session start agent --task-id <id>` | Enable stop hook |
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
| `frontend` | UI, components, CSS, a11y |
| `backend` | APIs, auth, validation |
| `data` | Pipelines, SQL, schemas |
| `test` | Coverage, fixtures, e2e |
| `infra` | CI/CD, containers, cloud |
| `review` | Quality, patterns |
| `security` | Vulnerabilities, OWASP |
| `generic` | Default |

Source of truth: `archetypes.yaml`. Project override: `.bacchus/archetypes.yaml`.

### Task Status Flow

```
draft → open → in_progress → ready_for_release → releasing → closed
                    ↓                                  ↓
                 failed                        needs_resolution
```

## Source Structure

```
src/
├── main.rs           # CLI entry, routing
├── tasks.rs          # SQLite task ops, YAML import
├── workspace.rs      # jj workspace ops
├── db/               # Schema, migrations
└── tools/
    ├── claim.rs      # Claim task
    ├── release.rs    # Release task
    ├── session.rs    # Stop hook integration
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
    CHECK (archetype IN ('frontend','backend','data','test','infra','review','security','generic'))
);
```

## Environment

| Variable | Purpose |
|----------|---------|
| `CLAUDE_PROJECT_DIR` | Workspace root (set by Claude Code) |
| `BACCHUS_DB_PATH` | Override DB location |

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
