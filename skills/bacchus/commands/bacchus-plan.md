# Bacchus Task Planner

Decompose a goal into a `.bacchus/tasks.yaml` file optimized for parallel agent execution.

## Arguments

`$ARGUMENTS` contains the goal to decompose.

## Steps

### 1. Understand the Codebase

Index and search for relevant code:
```bash
bacchus index src/
bacchus symbols --search "<relevant terms>" --handle
bacchus handle expand <handle> --limit 10
```

Read key files to understand architecture before planning tasks.

### 2. Design the Task Graph

Principles:
- **Maximize parallelism** — only add `depends_on` when a task truly needs another's output
- **Non-overlapping footprints** — concurrent tasks must not touch the same files/symbols
- **Right-sized tasks** — each task should be completable by one agent in one session
- **Clear boundaries** — each task owns specific files/symbols via its footprint

### 3. Write tasks.yaml

```yaml
tasks:
  - id: <PREFIX>-001
    title: "<imperative description>"
    task_type: feature|bug_fix|refactor|test|docs|infra|generic
    archetype: design|frontend|backend|data|test|infra|review|security|generic
    depends_on: []
    footprint:
      modifies:
        - "src/file.rs::SymbolName"
      creates:
        - "src/new_file.rs"
```

Footprint syntax:
- `src/file.rs` — entire file
- `src/file.rs::Symbol` — specific symbol
- `src/file.rs::*` — all symbols in file
- `src/dir/*.rs` — glob

### 4. Validate

```bash
bacchus task import --epic-id <EPIC_ID>
bacchus task list --ready
bacchus task validate
```

Fix any validation errors (overlapping footprints, missing deps, invalid symbols).

### 5. Output

Print a summary:
- Total tasks, how many can run in parallel initially
- Critical path (longest dependency chain)
- Recommended `--max-concurrent` value
