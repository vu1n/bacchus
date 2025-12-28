---
name: bacchus-planner
description: Break down complex requests into trackable beads with dependencies. Use before orchestrating work with bacchus.
---

# Bacchus Planner

Break down complex user requests into atomic, trackable beads with proper dependencies.

## Workflow

1. **Analyze**: Understand the user's request and current project state
2. **Decompose**: Split into atomic units (one PR per bead)
3. **Sequence**: Map dependencies (what blocks what)
4. **Create**: Build the beads using `bd` CLI

## Principles

- **Atomic**: Each bead = one logical change = one PR
- **Ordered**: Use `bd dep add` to enforce sequence
- **Testable**: Each bead should be independently verifiable
- **Parallelizable**: Independent beads can run concurrently

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
bd list --status=open

# Create beads
bd create --title="Add avatar_url to users table" --type=task --priority=1
# → Created beads-abc123

bd create --title="Avatar upload API endpoint" --type=task --priority=1
# → Created beads-def456

bd create --title="Profile page UI" --type=task --priority=2
# → Created beads-ghi789

bd create --title="Avatar upload component" --type=task --priority=2
# → Created beads-jkl012

# Set dependencies
bd dep add beads-def456 beads-abc123  # API needs schema
bd dep add beads-ghi789 beads-def456  # UI needs API
bd dep add beads-jkl012 beads-def456  # Upload needs API

# Verify
bd list --status=open
bd ready  # Should show beads-abc123 as only ready bead
```

## Checklist

Before finishing:
- [ ] Every significant unit of work has a bead
- [ ] Dependencies correctly model the execution order
- [ ] No circular dependencies
- [ ] Ready beads can be worked on immediately

## Next Steps

After planning, orchestrate the work:
```
/bacchus-orchestrate
```

Or work on a single bead:
```
/bacchus-agent <bead_id>
```
