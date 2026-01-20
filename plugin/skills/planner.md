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
4. **Create**: Build tasks in `.bacchus/tasks.yaml` and import to SQLite

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
# Initialize tasks file if needed
bacchus task init

# Edit .bacchus/tasks.yaml with your tasks (see format below)

# Import tasks to SQLite
bacchus task import --epic-id PROFILE

# Verify
bacchus task list
bacchus task list --ready  # Should show first task as ready
```

## Tasks YAML Format

Edit `.bacchus/tasks.yaml`:

```yaml
version: 1

tasks:
  - id: PROFILE-001
    title: "Add avatar_url to users table"
    description: "Add avatar_url column and migration"
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
    description: "POST /api/users/:id/avatar endpoint"
    priority: 1
    status: open
    depends_on: [PROFILE-001]
    footprint:
      modifies:
        - "src/api/users.rs::*"
      creates:
        - "src/api/avatar.rs"

  - id: PROFILE-003
    title: "Profile page UI"
    description: "Display user profile with avatar"
    priority: 2
    status: open
    depends_on: [PROFILE-002]
    footprint:
      creates:
        - "src/components/Profile.tsx"

  - id: PROFILE-004
    title: "Avatar upload component"
    description: "UI component for uploading/cropping avatar"
    priority: 2
    status: open
    depends_on: [PROFILE-002]
    footprint:
      creates:
        - "src/components/AvatarUpload.tsx"
```

## Commands Reference

```bash
# Initialize tasks.yaml template
bacchus task init

# Import tasks from YAML to SQLite
bacchus task import --epic-id <EPIC_ID>

# List all tasks
bacchus task list

# List only ready tasks
bacchus task list --ready

# Show task details
bacchus task show <task_id>

# Validate task definitions
bacchus task validate
```

## Checklist

Before finishing:
- [ ] Every significant unit of work has a task
- [ ] Dependencies correctly model the execution order
- [ ] No circular dependencies
- [ ] Ready tasks can be worked on immediately
- [ ] Tasks imported to SQLite with `bacchus task import`

## Next Steps

After planning, orchestrate the work:
```
/bacchus-orchestrate
```

Or work on a single task:
```
/bacchus-agent <task_id>
```
