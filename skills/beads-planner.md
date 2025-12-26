---
name: beads-planner
description: Planning helper for breaking down complex requests into trackable tasks using the `beads` CLI. Use this as a primitive before coordinating work with `bacchus`.
---

# Beads Planner

Use this skill to break down complex user requests into atomic, trackable tasks using the `beads` primitive.

## Workflow

1.  **Analyze**: Read the user's request and current `beads` state.
2.  **Breakdown**: Split the work into atomic units (implementation, testing, docs).
3.  **Dependency**: Map out which tasks block others.
4.  **Execute**: Create the beads using `beads` CLI.

## Guiding Principles

*   **Atomic**: Each bead should be one PR / one logical change.
*   **Trackable**: If it takes >1 hour, it needs a bead.
*   **Ordered**: Use dependencies (`beads dep`) to enforce order (e.g., Backend -> Frontend).

## Example Session

**User**: "Implement the user profile page with avatar upload."

**Planner Agent**:

1.  *Thinking*: This requires database changes, API endpoints, and a frontend page.
2.  *Action*:
    ```bash
    # Check existing context
    beads list

    # Create tasks
    beads add "Schema: Add avatar_url to users table" --tag db
    # Output: Created BEAD-10

    beads add "API: Upload endpoint for avatars" --tag api
    # Output: Created BEAD-11

    beads add "UI: User profile page structure" --tag ui
    # Output: Created BEAD-12

    beads add "UI: Avatar upload component" --tag ui
    # Output: Created BEAD-13

    # Link dependencies
    beads dep BEAD-11 BEAD-10  # API needs Schema
    beads dep BEAD-12 BEAD-11  # UI needs API (optional, but good for mocking)
    beads dep BEAD-13 BEAD-11  # Upload component needs API
    ```

## Checklist

- [ ] Does every new feature have a bead?
- [ ] Are dependencies correctly linked?
- [ ] Are tags used for filtering (e.g., `db`, `ui`, `api`)?
