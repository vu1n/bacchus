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


# Maps prompt type -> rubric. Extend as we add orchestrator/planner evals.
RUBRICS: dict[str, dict] = {
    "worker": WORKER_RUBRIC,
}


def get_rubric(prompt_type: str) -> dict:
    """Get the rubric for a prompt type."""
    if prompt_type not in RUBRICS:
        raise ValueError(
            f"No rubric for prompt type '{prompt_type}'. "
            f"Available: {list(RUBRICS.keys())}"
        )
    return RUBRICS[prompt_type]
