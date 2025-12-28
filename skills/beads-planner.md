---
name: beads-planner
description: Planning helper for breaking down complex requests into trackable tasks using the `bd` CLI. Use this as a primitive before coordinating work with `bacchus`.
---

# Beads Planner

Use this skill to break down complex user requests into atomic, trackable tasks using the `bd` CLI.

## Workflow

1.  **Analyze**: Read the user's request and current `bd` state.
2.  **Breakdown**: Split the work into atomic units (implementation, testing, docs).
3.  **Dependency**: Map out which tasks block others.
4.  **Execute**: Create the beads using the `bd` CLI.

## Guiding Principles

*   **Atomic**: Each bead should be one PR / one logical change.
*   **Trackable**: If it takes >1 hour, it needs a bead.
*   **Ordered**: Use dependencies (`bd dep add`) to enforce order (e.g., Backend -> Frontend).

## Example Session

**User**: "Implement the user profile page with avatar upload."

**Planner Agent**:

1.  *Thinking*: This requires database changes, API endpoints, and a frontend page.
2.  *Action*:
    ```bash
    # Check existing context
    bd list

    # Create tasks
    bd create "Schema: Add avatar_url to users table" --labels db
    # Output: Created BEAD-10

    bd create "API: Upload endpoint for avatars" --labels api
    # Output: Created BEAD-11

    bd create "UI: User profile page structure" --labels ui
    # Output: Created BEAD-12

    bd create "UI: Avatar upload component" --labels ui
    # Output: Created BEAD-13

    # Link dependencies
    bd dep add BEAD-11 BEAD-10  # API needs Schema
    bd dep add BEAD-12 BEAD-11  # UI needs API (optional, but good for mocking)
    bd dep add BEAD-13 BEAD-11  # Upload component needs API
    ```

## Checklist

- [ ] Does every new feature have a bead?
- [ ] Are dependencies correctly linked?
- [ ] Are tags used for filtering (e.g., `db`, `ui`, `api`)?
