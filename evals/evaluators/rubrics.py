"""Behavioral rubrics for bacchus prompt evaluation.

Each rubric defines weighted categories of expected behavior.
Categories contain checks: human-readable descriptions of what correct
behavior looks like. The proxy evaluator scores transcripts against these.
"""

WORKER_RUBRIC: dict = {
    "session_management": {
        "weight": 0.10,
        "checks": [
            "Calls `bacchus session start agent` with correct --agent-id and --task-id",
            "Calls `bacchus claim` or `bacchus next` to acquire the task",
        ],
    },
    "context_loading": {
        "weight": 0.10,
        "checks": [
            "Calls `bacchus task show` or `bacchus context` to understand the task",
            "Calls `bacchus archetype prompt` for the task's archetype (or notes it's already injected)",
        ],
    },
    "workspace_discipline": {
        "weight": 0.20,
        "checks": [
            "Uses `jj -R .bacchus/workspaces/<TASK_ID>/` for all VCS commands",
            "Never uses `cd` to enter the workspace directory",
            "Edits files via full paths (.bacchus/workspaces/<TASK_ID>/...)",
        ],
    },
    "footprint_compliance": {
        "weight": 0.15,
        "checks": [
            "Only modifies files listed in footprint.modifies",
            "Only creates files listed in footprint.creates",
            "When needing out-of-footprint changes, releases as blocked instead of proceeding",
        ],
    },
    "review_loop": {
        "weight": 0.20,
        "checks": [
            "Rebases onto main before releasing: `jj -R ... rebase -d main`",
            "Runs /simplify to clean up code",
            "Runs `bacchus review <TASK_ID>` to validate changes",
            "Fixes issues if review fails, then re-runs the loop",
            "Does NOT release as done until review passes",
        ],
    },
    "release_protocol": {
        "weight": 0.15,
        "checks": [
            "Calls `jj -R ... describe -m` to set a commit message before release",
            "Calls `bacchus release <TASK_ID> --status done` on success",
            "Uses --status blocked or --status failed when appropriate for the scenario",
        ],
    },
    "constraints": {
        "weight": 0.10,
        "checks": [
            "Never runs bun/npm/pnpm/yarn install commands",
            "Sends heartbeats during long work phases (curl or bacchus activity)",
        ],
    },
}


ORCHESTRATOR_RUBRIC: dict = {
    "initialization": {
        "weight": 0.10,
        "checks": [
            "Calls `bacchus init` to bootstrap the project",
            "Calls `bacchus epic create` with a meaningful --title and --description",
            "Notes the EPIC_ID from the output for later use",
        ],
    },
    "task_planning": {
        "weight": 0.20,
        "checks": [
            "Creates `.bacchus/tasks.yaml` with well-structured tasks",
            "Each task has id, title, description, task_type, archetype, depends_on, and footprint",
            "Task descriptions are detailed enough for a worker to execute without reading plan docs",
            "Footprints are non-overlapping for concurrent tasks",
            "Dependency graph maximizes parallelism (only adds depends_on when truly needed)",
            "Runs `bacchus task import --epic-id <EPIC_ID>` after writing tasks.yaml",
            "Runs `bacchus task validate` to check footprints",
            "Includes test-first instructions for high-impact feature/refactor tasks",
        ],
    },
    "session_management": {
        "weight": 0.05,
        "checks": [
            "Calls `bacchus session start orchestrator` with --max-concurrent, --epic-id, and --goal",
        ],
    },
    "worker_spawning": {
        "weight": 0.15,
        "checks": [
            "Spawns workers via `bacchus session spawn-workers --count N`",
            "Uses --dry-run before first spawn to preview",
            "Never uses the Agent tool to spawn workers",
            "Spawns replacement workers when ready tasks exist after releases/recovery",
        ],
    },
    "monitor_loop": {
        "weight": 0.15,
        "checks": [
            "Runs `bacchus status` to check overall progress",
            "Runs `bacchus list` to see active claims",
            "Runs `bacchus process-releases` to merge completed work",
            "Runs `bacchus stale --minutes 15 --cleanup` to recover dead workers",
            "Checks events with `bacchus events --limit 20`",
            "Checks messages with `bacchus message list --agent orchestrator`",
            "Checks `bacchus task list --ready` and spawns workers for newly ready tasks",
            "Respects polling cadence (~30s between iterations, back off to ~60s on no changes)",
        ],
    },
    "recovery": {
        "weight": 0.15,
        "checks": [
            "Handles stale workers by running stale --cleanup and spawning replacements",
            "Handles failed tasks by allowing retry (task resets to open)",
            "Stops retrying after 3 failures and triggers re-planning",
            "Handles merge conflicts by messaging the responsible agent",
            "Handles blocked workers by checking messages and adjusting tasks/footprints",
        ],
    },
    "release_processing": {
        "weight": 0.10,
        "checks": [
            "Runs `bacchus process-releases` frequently to unblock downstream tasks",
            "Runs dependency install (bun/npm/cargo) after releases that add dependencies",
            "Workers are instructed NOT to run package install (orchestrator handles it)",
        ],
    },
    "hard_rules": {
        "weight": 0.10,
        "checks": [
            "Never writes, edits, or creates source code files (anything outside .bacchus/)",
            "Never runs `bacchus claim`, `bacchus next`, or `bacchus release`",
            "Never edits files in `.bacchus/workspaces/`",
            "Only output artifacts are `.bacchus/tasks.yaml` and messages to workers",
            "Never uses the Agent tool to spawn workers (uses `bacchus session spawn-workers`)",
            "Never uses git worktrees (bacchus uses jj workspaces)",
        ],
    },
}


# Maps prompt type -> rubric. Extend as we add planner evals.
RUBRICS: dict[str, dict] = {
    "worker": WORKER_RUBRIC,
    "orchestrator": ORCHESTRATOR_RUBRIC,
}


def get_rubric(prompt_type: str) -> dict:
    """Get the rubric for a prompt type."""
    if prompt_type not in RUBRICS:
        raise ValueError(
            f"No rubric for prompt type '{prompt_type}'. "
            f"Available: {list(RUBRICS.keys())}"
        )
    return RUBRICS[prompt_type]
