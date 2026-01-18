---
name: bacchus-planner
description: Break down complex requests into trackable tasks with dependencies. Use before orchestrating work with bacchus.
---

# Bacchus Planner

Break down complex user requests into atomic, trackable tasks with proper dependencies.

## Workflow

1. **Analyze**: Understand the user's request and current project state
2. **Decompose**: Split into atomic units (one PR per task)
3. **Sequence**: Map dependencies (what blocks what)
4. **Create**: Build the tasks in `.bacchus/tasks.yaml` or use CLI

## Principles

- **Atomic**: Each task = one logical change = one PR
- **Ordered**: Use `depends_on` to enforce sequence
- **Testable**: Each task should be independently verifiable
- **Parallelizable**: Independent tasks can run concurrently

## Example

**Request**: "Implement user profile with avatar upload"

**Decomposition**:
```
Schema (db)
    ↓
API endpoints (api)
    ↓
UI components (ui)
```

**Execution**:
```bash
# Check current state
bacchus task list

# Initialize tasks file if needed
bacchus task init

# Add tasks with dependencies
bacchus task add --id PROFILE-001 --title "Add avatar_url to users table" --priority 1
bacchus task add --id PROFILE-002 --title "Avatar upload API endpoint" --priority 1 --deps PROFILE-001
bacchus task add --id PROFILE-003 --title "Profile page UI" --priority 2 --deps PROFILE-002
bacchus task add --id PROFILE-004 --title "Avatar upload component" --priority 2 --deps PROFILE-002

# Verify
bacchus task list
bacchus task list --ready  # Should show PROFILE-001 as only ready task
```

## Tasks YAML Format

You can also directly edit `.bacchus/tasks.yaml`:

```yaml
version: 1
tasks:
  - id: PROFILE-001
    title: "Add avatar_url to users table"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies:
        - "src/db/schema.rs::UserTable"
      creates:
        - "migrations/add_avatar_url.sql"

  - id: PROFILE-002
    title: "Avatar upload API endpoint"
    priority: 1
    status: open
    depends_on: [PROFILE-001]
    footprint:
      modifies:
        - "src/api/users.rs::*"
      creates:
        - "src/api/avatar.rs"
```

## Checklist

Before finishing:
- [ ] Every significant unit of work has a task
- [ ] Dependencies correctly model the execution order
- [ ] No circular dependencies
- [ ] Ready tasks can be worked on immediately

## Next Steps

After planning, orchestrate the work:
```
/bacchus-orchestrate
```

Or work on a single task:
```
/bacchus-agent <task_id>
```
