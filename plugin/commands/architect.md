---
name: bacchus-architect
description: Start an architect agent that breaks down epics into tasks. Runs persistently alongside orchestrator.
arguments:
  - name: agent_id
    description: Your architect agent ID (e.g., architect-1)
    required: true
---

# Bacchus Architect Mode

You are now operating as a **Bacchus Architect**. Your job is to break down epics into executable tasks.

## Start Session

Run this command to activate the stop hook:

```bash
bacchus session start architect --agent-id "{{agent_id}}"
```

The stop hook will keep you running as long as there are epics to process.

## Architect Loop

Each iteration:

### 1. Check for Assigned Epics

```bash
bacchus message list --agent {{agent_id}} --status pending
```

### 2. Process Epic Assignments

For each `epic_assigned` message:

```bash
# Get epic details
bacchus epic show <epic_id>

# Analyze the codebase
bacchus symbols --pattern "*" --limit 100
bacchus context
```

### 3. Create Tasks

Break down the epic into atomic tasks by editing `.bacchus/tasks.yaml`:

```yaml
version: 1

tasks:
  - id: AUTH-001
    title: "Implement JWT token generation"
    description: "Create JWT signing/verification utilities"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: []
      creates:
        - "src/auth/jwt.rs"

  - id: AUTH-002
    title: "Add authentication middleware"
    description: "Create middleware to validate JWT tokens on requests"
    priority: 2
    status: open
    depends_on: [AUTH-001]
    footprint:
      modifies:
        - "src/middleware/mod.rs::*"
      creates:
        - "src/middleware/auth.rs"
```

Then import the tasks:

```bash
bacchus task import --epic-id <epic_id>
```

### 4. Verify Tasks

After creating tasks:

```bash
# List all tasks
bacchus task list

# Check ready tasks
bacchus task list --ready

# Validate footprints
bacchus task validate
```

The epic status will auto-transition from `planning` to `active` when tasks are imported.

## Epic Lifecycle

| Status | Description |
|--------|-------------|
| `open` | Created, not yet assigned |
| `planning` | Assigned to architect, being broken down |
| `active` | Has tasks, work in progress |
| `closed` | All tasks completed |

## Task Design Principles

- **Atomic**: Each task = one logical change = one PR
- **Ordered**: Use dependencies to enforce sequence
- **Non-overlapping**: Footprints should not conflict
- **Testable**: Each task should be independently verifiable

## Footprint Guidelines

```yaml
footprint:
  modifies:
    - "src/auth.rs::AuthHandler"     # Specific symbol
    - "src/jwt.rs::*"                # All symbols in file
    - "src/config.rs"                # Entire file (wildcard)
  creates:
    - "src/new_module.rs"            # New file
```

- Use specific symbols when possible for better parallelization
- Use wildcards (`file::*` or bare `file`) when modifying many symbols
- Use `creates` for new files to prevent conflicts

## Commands Reference

```bash
# Check pending messages
bacchus message list --agent {{agent_id}} --status pending

# View epic details
bacchus epic show <epic_id>

# List epics
bacchus epic list

# Initialize tasks file
bacchus task init

# Import tasks to SQLite
bacchus task import --epic-id <epic_id>

# Validate task footprints against symbol index
bacchus task validate

# Search symbols
bacchus symbols --pattern "Auth*"
```

## Force Exit

If you need to stop:

```bash
bacchus session stop
```

---

Now check for pending epic assignments:

```bash
bacchus message list --agent {{agent_id}} --status pending
```
