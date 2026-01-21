---
name: bacchus
description: Multi-agent coordination for parallel development. Use when asked to parallelize work, coordinate multiple agents, or manage tasks across a codebase.
---

# Bacchus: Multi-Agent Coordination

Bacchus coordinates parallel agent work on codebases using isolated jj workspaces.

## Quick Start

For parallel work: **plan** -> **import** -> **orchestrate**

```bash
# 1. Initialize tasks file
bacchus task init

# 2. Edit .bacchus/tasks.yaml with your task breakdown

# 3. Import to SQLite
bacchus task import --epic-id <EPIC>

# 4. Check ready tasks
bacchus task list --ready
```

## Workflow Commands

| Command | Purpose |
|---------|---------|
| `bacchus task init` | Create tasks.yaml template |
| `bacchus task import --epic-id X` | Import to SQLite |
| `bacchus task list [--ready]` | List tasks |
| `bacchus task show <id>` | Show task details |
| `bacchus claim <id> <agent>` | Claim a task |
| `bacchus release <id> --status done` | Complete task |
| `bacchus session start agent --task-id X` | Start agent session |
| `bacchus session start orchestrator` | Start orchestrator session |
| `bacchus session stop` | Clear session |

## Planning Tasks

Create `.bacchus/tasks.yaml`:

```yaml
version: 1

tasks:
  # Implementation tasks
  - id: TASK-001
    title: "Add login endpoint"
    type: backend        # frontend | backend | data | test | infra | generic
    priority: 1
    status: open
    depends_on: []
    footprint:
      creates: ["src/api/login.rs"]
      modifies: ["src/api/mod.rs::*"]

  - id: TASK-002
    title: "Add login form component"
    type: frontend
    priority: 2
    depends_on: [TASK-001]
    footprint:
      creates: ["src/components/LoginForm.tsx"]

  # Review tasks (depend on implementation)
  - id: TASK-001-SECURITY
    title: "Security review of login endpoint"
    type: security
    priority: 3
    depends_on: [TASK-001]

  - id: TASK-002-REVIEW
    title: "Code review of login form"
    type: review
    priority: 3
    depends_on: [TASK-002]
```

## Archetype System

Bacchus uses archetypes to provide specialized prompts for different task types. Archetypes are defined in `archetypes.yaml`.

### Archetype Commands

```bash
# List available archetypes
bacchus archetype list

# Show archetype details
bacchus archetype show frontend

# Get the prompt for an archetype
bacchus archetype prompt security

# Select best archetype for a task
bacchus archetype select TASK-001
```

### Available Archetypes

| Type | Archetype | Focus |
|------|-----------|-------|
| frontend | Frontend Design | UI/UX, components, styling, accessibility |
| backend | Backend API | APIs, auth, validation, error handling |
| data | Data Engineer | Pipelines, SQL, schemas, ETL |
| test | Test Engineer | Coverage, fixtures, e2e, mocks |
| infra | Infrastructure | CI/CD, containers, cloud, monitoring |
| review | Code Reviewer | Quality, patterns, correctness |
| security | Security Specialist | Vulnerabilities, OWASP, secrets |
| generic | Generic | General development (default) |

### Customizing Archetypes

Copy `archetypes.yaml` to `.bacchus/archetypes.yaml` in your project to customize:

```bash
cp ~/.claude/skills/bacchus/archetypes.yaml .bacchus/archetypes.yaml
# Edit .bacchus/archetypes.yaml as needed
```

## Orchestrator Mode

When orchestrating, spawn agents using the Task tool with archetype-specific prompts.

### Spawning Agents

1. Get the archetype for a task:
```bash
bacchus archetype select TASK-001
```

2. Use the Task tool with the archetype prompt:
```
Task tool:
  subagent_type: "general-purpose"
  run_in_background: true
  prompt: |
    [Archetype prompt from bacchus archetype prompt <type>]

    ## Your Task

    Task ID: {task_id}
    Title: {title}
    Description: {description}

    ## Setup

    Start your session and claim the task:
    ```bash
    bacchus session start agent --task-id "{task_id}"
    bacchus claim "{task_id}" agent-{unique_id}
    ```

    ## Work in Workspace

    IMPORTANT: Never `cd` into the workspace. Use `jj -R` flag instead.

    ```bash
    jj -R .bacchus/workspaces/{task_id} status
    jj -R .bacchus/workspaces/{task_id} describe -m "Your commit message"
    jj -R .bacchus/workspaces/{task_id} diff
    ```

    ## Complete

    When done:
    ```bash
    bacchus release {task_id} --status done
    ```
```

## Session Management

Sessions enable stop hooks that prevent premature exit:

```bash
# Agent mode - blocks until task is closed
bacchus session start agent --task-id TASK-42

# Orchestrator mode - blocks while work remains
bacchus session start orchestrator --max-concurrent 3

# Check status
bacchus session status

# Clear session to allow exit
bacchus session stop
```

## Orchestrator Loop

Each iteration:

1. **Check status**: `bacchus task list && bacchus task list --ready && bacchus list`
2. **Select archetypes**: `bacchus archetype select <task_id>` for each ready task
3. **Spawn agents** for ready tasks (up to max_concurrent) with appropriate archetype prompts
4. **Monitor progress**: Check for completed/failed agents
5. **Handle releases**: Tasks marked `ready_for_release` get merged by orchestrator
6. **Cleanup stale work**: `bacchus stale --minutes 30 --cleanup`

## Conflict Resolution

If the orchestrator detects merge conflicts:
1. Task is marked `needs_resolution`
2. Agent resolves: `jj -R .bacchus/workspaces/{task_id} resolve`
3. Agent marks resolved: `bacchus resolve {task_id}`

## Force Exit

To exit without completing:

```bash
bacchus session stop
```

---

## Example: Full Workflow

```bash
# User asks to implement authentication with multiple agents

# 1. Plan the work - create tasks with types and reviews
bacchus task init
# Edit .bacchus/tasks.yaml with implementation + review tasks

# 2. Import tasks
bacchus task import --epic-id AUTH

# 3. Start orchestrator session
bacchus session start orchestrator --max-concurrent 3

# 4. Check ready tasks and their archetypes
bacchus task list --ready
bacchus archetype select AUTH-001  # Shows: backend archetype

# 5. Spawn agents with appropriate archetypes
# (use Task tool with archetype prompts)

# 6. Monitor and wait for completion
bacchus task list
bacchus list  # Active claims

# 7. All done - session auto-clears or:
bacchus session stop
```
