"""LLM-as-judge proxy evaluator for bacchus prompts.

Given a candidate prompt and a task scenario, simulates what a worker agent
would do, then scores the simulated transcript against the behavioral rubric.

Uses `claude -p` (Claude Code pipe mode) to leverage the user's subscription
instead of requiring an API key.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys

import yaml

from .rubrics import ORCHESTRATOR_RUBRIC, PLANNER_RUBRIC, WORKER_RUBRIC

CLAUDE_BIN = os.environ.get("CLAUDE_BIN", "claude")
DEFAULT_MODEL = os.environ.get("BACCHUS_MODEL", "sonnet")

# Pre-compiled regex patterns for planner description scoring
_RE_FILE_PATHS = [re.compile(p) for p in [r"src/", r"\.\w{1,4}", r"packages/", r"apps/"]]
_RE_SYMBOLS = [re.compile(p) for p in [
    r"[A-Z][a-z]+[A-Z]", r"[a-z]+_[a-z]+", r"`[a-zA-Z_]\w*`", r"::[A-Za-z]",
]]
_RE_LINE_NUMBERS = [re.compile(p, re.IGNORECASE) for p in [r"line \d+", r"L\d+", r":\d{2,}"]]
_RE_GATES = [re.compile(p, re.IGNORECASE) for p in [
    r"done when", r"gate:", r"verify:", r"must pass", r"acceptance",
    r"ensure that", r"should (pass|succeed|compile|build)", r"tests? (pass|green|succeed)",
]]
_RE_PATTERNS = [re.compile(p, re.IGNORECASE) for p in [
    r"follow.{0,20}pattern", r"see.{0,20}example", r"similar to",
    r"same approach as", r"modeled after", r"consistent with",
]]
_RE_NO_INSTALL = [re.compile(p, re.IGNORECASE) for p in [
    r"do not.{0,20}install", r"don't.{0,20}install", r"no.{0,20}install",
    r"must not.{0,20}install", r"never.{0,20}install", r"orchestrator.{0,30}install",
]]


def _claude_prompt(prompt: str, system: str | None = None, model: str | None = None) -> str:
    """Call claude -p with a prompt string, return raw text output."""
    # Combine system + user into a single prompt for claude -p
    if system:
        full_prompt = f"<instructions>\n{system}\n</instructions>\n\n{prompt}"
    else:
        full_prompt = prompt

    model = model or DEFAULT_MODEL
    cmd = [CLAUDE_BIN, "-p", "--model", model, "--max-turns", "1"]

    # Strip CLAUDECODE env var to allow nested claude -p invocations
    env = {k: v for k, v in os.environ.items() if k != "CLAUDECODE"}

    result = subprocess.run(
        cmd,
        input=full_prompt,
        capture_output=True,
        text=True,
        timeout=300,
        env=env,
    )
    if result.returncode != 0:
        print(f"  claude -p stderr: {result.stderr[:500]}", file=sys.stderr)
        raise RuntimeError(f"claude -p failed (exit {result.returncode}): {result.stderr[:200]}")
    return result.stdout


def _parse_json(text: str) -> dict:
    """Extract a JSON object from LLM text output."""
    # Try ```json fenced block first
    json_match = re.search(r"```json\s*(.*?)\s*```", text, re.DOTALL)
    if json_match:
        try:
            return json.loads(json_match.group(1))
        except json.JSONDecodeError:
            pass

    # Try raw JSON parse
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        pass

    # Try to find any JSON object in the response
    obj_match = re.search(r"\{.*\}", text, re.DOTALL)
    if obj_match:
        try:
            return json.loads(obj_match.group())
        except json.JSONDecodeError:
            pass

    return {}


# ---------------------------------------------------------------------------
# Simulation: ask an LLM to role-play as the worker
# ---------------------------------------------------------------------------

_SIMULATION_SYSTEM = """\
You are simulating a Claude Code agent that has been given a worker prompt and a task.
Your job is to produce a detailed, realistic transcript of exactly what commands the agent
would run and what files it would create/modify, in order.

Output a JSON object with:
- "plan": array of step objects, each with:
  - "action": "bash" | "edit" | "create" | "read"
  - "command": the exact command string (for bash) or file path (for edit/create/read)
  - "reasoning": why this step is taken
- "release_status": "done" | "blocked" | "failed"
- "release_reason": brief explanation of release decision

Be precise about command arguments and file paths. Include ALL commands the agent
would run, including session management, heartbeats, review loops, etc.
Do NOT skip steps or summarize — list every command.
Output ONLY the JSON object, no other text."""


def build_simulation_prompt(candidate_prompt: str, scenario: dict) -> str:
    """Build the user message for the simulation LLM call."""
    parts = [
        "## Worker Prompt (the agent's instructions)\n",
        candidate_prompt,
        "\n\n## Task Scenario\n",
        f"**Task ID:** {scenario['task_id']}\n",
        f"**Title:** {scenario['task_title']}\n",
        f"**Type:** {scenario['task_type']}\n",
        f"**Archetype:** {scenario['archetype']}\n",
        f"**Description:** {scenario.get('task_description', 'N/A')}\n",
    ]

    footprint = scenario.get("footprint") or {}
    if footprint:
        parts.append("\n**Footprint:**\n")
        if footprint.get("creates"):
            parts.append(f"  creates: {json.dumps(footprint['creates'])}\n")
        if footprint.get("modifies"):
            parts.append(f"  modifies: {json.dumps(footprint['modifies'])}\n")

    if scenario.get("context_output"):
        parts.append(f"\n**Context output:**\n{scenario['context_output']}\n")

    if scenario.get("special_conditions"):
        parts.append(
            f"\n**Special conditions:** {scenario['special_conditions']}\n"
        )

    parts.append(
        "\n---\n"
        "Simulate this agent executing the task. Produce the full command transcript as JSON."
    )
    return "".join(parts)


def run_simulation(prompt: str, model: str | None = None) -> dict:
    """Run the simulation via claude -p and parse the JSON transcript."""
    text = _claude_prompt(prompt, system=_SIMULATION_SYSTEM, model=model)
    result = _parse_json(text)
    if not result:
        return {"plan": [], "release_status": "unknown", "error": "Failed to parse simulation"}
    return result


# ---------------------------------------------------------------------------
# Scoring: check the transcript against the rubric
# ---------------------------------------------------------------------------

_SCORING_SYSTEM = """\
You are a strict evaluator scoring an agent transcript against a behavioral rubric.

For each rubric check, determine if the transcript demonstrates that behavior.
Output a JSON object where each key is a rubric category name, containing:
- "score": float 0.0-1.0 (fraction of checks passed in this category)
- "passed_checks": list of check descriptions that passed
- "failed_checks": list of check descriptions that failed
- "reason": brief explanation

Be strict: if a command is missing or wrong, the check fails.
Partial credit (0.5) only if the intent is clearly present but execution is imperfect.
Output ONLY the JSON object, no other text."""


def score_transcript(
    transcript: dict,
    scenario_rubric: dict,
    rubric: dict = WORKER_RUBRIC,
    model: str | None = None,
) -> tuple[float, dict]:
    """Score a simulated transcript against the rubric.

    Returns (weighted_score, category_details).
    """
    scoring_prompt = json.dumps(
        {
            "transcript": transcript,
            "scenario_rubric": scenario_rubric,
            "rubric": {
                cat: {"checks": data["checks"]}
                for cat, data in rubric.items()
            },
        },
        indent=2,
    )

    text = _claude_prompt(scoring_prompt, system=_SCORING_SYSTEM, model=model)
    details = _parse_json(text)

    if not details:
        details = {
            cat: {"score": 0.0, "reason": "Scoring parse failure"}
            for cat in rubric
        }

    # Compute weighted score
    total = 0.0
    for category, config in rubric.items():
        cat_score = details.get(category, {}).get("score", 0.0)
        total += cat_score * config["weight"]

    return total, details


# ---------------------------------------------------------------------------
# Public evaluator interface (matches optimize_anything signature)
# ---------------------------------------------------------------------------


def evaluate_worker(candidate_prompt: str, example: dict) -> tuple[float, dict]:
    """Proxy evaluator for optimize_anything.

    1. Simulate worker behavior given the candidate prompt + scenario
    2. Score the simulation against the behavioral rubric
    3. Return (score, details) for optimize_anything's reflection

    Args:
        candidate_prompt: The worker prompt text being evaluated.
        example: A scenario dict from cases/worker/scenarios.json.

    Returns:
        (score, details) where score is 0.0-1.0 and details is per-category.
    """
    sim_model = os.environ.get("BACCHUS_SIM_MODEL", None)
    score_model = os.environ.get("BACCHUS_SCORE_MODEL", None)

    # Step 1: Simulate
    sim_prompt = build_simulation_prompt(candidate_prompt, example)
    transcript = run_simulation(sim_prompt, model=sim_model)

    # Step 2: Score
    score, details = score_transcript(
        transcript,
        example.get("rubric", {}),
        model=score_model,
    )

    return score, details


# ---------------------------------------------------------------------------
# Orchestrator simulation + evaluation
# ---------------------------------------------------------------------------

_ORCHESTRATOR_SIMULATION_SYSTEM = """\
You are simulating a Claude Code agent that has been given an orchestrator prompt and an epic goal.
Produce a JSON object listing every shell command the agent would run, in order.

Output format:
{
  "commands": ["bacchus init", "bacchus epic create --title ...", ...],
  "files_written": [".bacchus/tasks.yaml"],
  "tasks_yaml_content": "brief summary of task structure (ids, types, archetypes, footprints, deps)",
  "violations": []
}

Rules:
- List EXACT shell commands with arguments (bacchus init, bacchus epic create, bacchus task import, etc.)
- Include monitor loop commands (bacchus status, bacchus list, process-releases, stale, events, message list, task list --ready, spawn-workers)
- Include session start and finalize commands
- violations: list any rule violations (writing source code, claiming tasks, using Agent tool, using git worktrees)
- Be exhaustive — list every command the agent would run
- Output ONLY the JSON object, no other text."""


def build_orchestrator_simulation_prompt(candidate_prompt: str, scenario: dict) -> str:
    """Build the user message for orchestrator simulation."""
    parts = [
        "## Orchestrator Prompt (the agent's instructions)\n",
        candidate_prompt,
        "\n\n## Epic Scenario\n",
        f"**Goal:** {scenario['epic_goal']}\n",
        f"**Type:** {scenario['epic_type']}\n",
        f"**Expected tasks:** ~{scenario['num_expected_tasks']}\n",
        f"**Archetypes needed:** {json.dumps(scenario['archetypes_needed'])}\n",
    ]

    if scenario.get("context_output"):
        parts.append(f"\n**Context (codebase state):**\n{scenario['context_output']}\n")

    if scenario.get("special_conditions"):
        parts.append(
            f"\n**Special conditions:** {scenario['special_conditions']}\n"
        )

    parts.append(
        "\n---\n"
        "Simulate this orchestrator agent executing the epic. "
        "Show the full command transcript as JSON, including all phases "
        "(init, plan, session start, spawn workers, monitor loop, finalize)."
    )
    return "".join(parts)


def run_orchestrator_simulation(prompt: str, model: str | None = None) -> dict:
    """Run orchestrator simulation via claude -p and parse the JSON transcript."""
    try:
        text = _claude_prompt(prompt, system=_ORCHESTRATOR_SIMULATION_SYSTEM, model=model)
    except subprocess.TimeoutExpired:
        print("  [TIMEOUT] Simulation timed out", file=sys.stderr)
        return {"commands": [], "violations": ["timeout"], "error": "Simulation timed out"}
    result = _parse_json(text)
    if not result:
        return {"commands": [], "violations": ["parse_failure"], "error": "Failed to parse simulation"}
    return result


def _get_commands(transcript: dict) -> list[str]:
    """Extract all command strings from a simulation transcript."""
    # New format: flat "commands" array
    if "commands" in transcript:
        return [c for c in transcript["commands"] if isinstance(c, str)]
    # Legacy format: "plan" array with step objects
    commands = []
    for step in transcript.get("plan", []):
        cmd = step.get("command", "")
        if cmd:
            commands.append(cmd)
    return commands


def _commands_contain(commands: list[str], *patterns: str) -> bool:
    """Check if any command matches ALL patterns (substring match)."""
    for cmd in commands:
        if all(p in cmd for p in patterns):
            return True
    return False


def _commands_contain_any(commands: list[str], *patterns: str) -> bool:
    """Check if any command matches ANY of the patterns."""
    for cmd in commands:
        for p in patterns:
            if p in cmd:
                return True
    return False


def _files_written(transcript: dict) -> list[str]:
    """Get list of files the orchestrator wrote."""
    explicit = transcript.get("files_written", [])
    written = []
    for step in transcript.get("plan", []):
        if step.get("action") in ("write_file", "create", "edit"):
            written.append(step.get("command", ""))
    return explicit + written


def score_orchestrator_deterministic(
    transcript: dict,
    scenario: dict,
) -> tuple[float, dict]:
    """Score an orchestrator transcript deterministically via pattern matching.

    Returns (weighted_score, category_details).
    """
    commands = _get_commands(transcript)
    files = _files_written(transcript)
    rubric_checks = scenario.get("rubric", {})
    # Full transcript as text for fuzzy keyword matching
    transcript_text = json.dumps(transcript).lower()

    details: dict[str, dict] = {}

    # --- initialization ---
    checks_init = ORCHESTRATOR_RUBRIC["initialization"]["checks"]
    passed, failed = [], []
    if _commands_contain(commands, "bacchus", "init"):
        passed.append(checks_init[0])
    else:
        failed.append(checks_init[0])
    if _commands_contain(commands, "bacchus", "epic", "create"):
        passed.append(checks_init[1])
    else:
        failed.append(checks_init[1])
    # "Notes EPIC_ID" — check if transcript mentions epic_id
    if any(kw in transcript_text for kw in ["epic_id", "epic id", "note", "store", "save"]):
        passed.append(checks_init[2])
    else:
        failed.append(checks_init[2])
    cat_score = len(passed) / max(len(checks_init), 1)
    details["initialization"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_init)} init checks passed",
    }

    # --- task_planning ---
    checks_plan = ORCHESTRATOR_RUBRIC["task_planning"]["checks"]
    passed, failed = [], []
    # Writes tasks.yaml
    if any("tasks.yaml" in f for f in files) or _commands_contain(commands, "tasks.yaml"):
        passed.append(checks_plan[0])
    else:
        failed.append(checks_plan[0])
    # Task structure (check if transcript mentions task fields)
    task_fields = ["id", "title", "description", "task_type", "archetype", "depends_on", "footprint"]
    if sum(1 for f in task_fields if f in transcript_text.lower()) >= 4:
        passed.append(checks_plan[1])
    else:
        failed.append(checks_plan[1])
    # Detailed descriptions
    if "description" in transcript_text.lower():
        passed.append(checks_plan[2])
    else:
        failed.append(checks_plan[2])
    # Non-overlapping footprints
    if "footprint" in transcript_text.lower() or "non-overlap" in transcript_text.lower():
        passed.append(checks_plan[3])
    else:
        failed.append(checks_plan[3])
    # Maximizes parallelism
    if any(kw in transcript_text.lower() for kw in ["parallel", "depends_on", "dependency"]):
        passed.append(checks_plan[4])
    else:
        failed.append(checks_plan[4])
    # Runs task import
    if _commands_contain(commands, "bacchus", "task", "import"):
        passed.append(checks_plan[5])
    else:
        failed.append(checks_plan[5])
    # Runs task validate
    if _commands_contain(commands, "bacchus", "task", "validate"):
        passed.append(checks_plan[6])
    else:
        failed.append(checks_plan[6])
    # Test-first instructions
    if rubric_checks.get("includes_test_first"):
        if any(kw in transcript_text.lower() for kw in ["test-first", "test first", "write tests before"]):
            passed.append(checks_plan[7])
        else:
            failed.append(checks_plan[7])
    else:
        passed.append(checks_plan[7])  # N/A for this scenario
    cat_score = len(passed) / max(len(checks_plan), 1)
    details["task_planning"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_plan)} planning checks passed",
    }

    # --- session_management ---
    checks_sess = ORCHESTRATOR_RUBRIC["session_management"]["checks"]
    passed, failed = [], []
    if _commands_contain(commands, "bacchus", "session", "start", "orchestrator"):
        passed.append(checks_sess[0])
    else:
        failed.append(checks_sess[0])
    cat_score = len(passed) / max(len(checks_sess), 1)
    details["session_management"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_sess)} session checks passed",
    }

    # --- worker_spawning ---
    checks_spawn = ORCHESTRATOR_RUBRIC["worker_spawning"]["checks"]
    passed, failed = [], []
    if _commands_contain(commands, "bacchus", "session", "spawn-workers"):
        passed.append(checks_spawn[0])
    else:
        failed.append(checks_spawn[0])
    if _commands_contain(commands, "spawn-workers", "--dry-run"):
        passed.append(checks_spawn[1])
    else:
        failed.append(checks_spawn[1])
    # Never uses Agent tool
    agent_tool_used = any(
        kw in transcript_text.lower()
        for kw in ["agent tool", "agent(", "spawn_agent"]
    )
    if not agent_tool_used:
        passed.append(checks_spawn[2])
    else:
        failed.append(checks_spawn[2])
    # Spawns replacements after recovery
    spawn_count = sum(1 for c in commands if "spawn-workers" in c)
    if spawn_count >= 2 or not rubric_checks.get("spawns_replacement_worker"):
        passed.append(checks_spawn[3])
    else:
        failed.append(checks_spawn[3])
    cat_score = len(passed) / max(len(checks_spawn), 1)
    details["worker_spawning"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_spawn)} spawn checks passed",
    }

    # --- monitor_loop ---
    checks_mon = ORCHESTRATOR_RUBRIC["monitor_loop"]["checks"]
    passed, failed = [], []
    if _commands_contain(commands, "bacchus", "status"):
        passed.append(checks_mon[0])
    else:
        failed.append(checks_mon[0])
    if _commands_contain(commands, "bacchus", "list"):
        passed.append(checks_mon[1])
    else:
        failed.append(checks_mon[1])
    if _commands_contain(commands, "bacchus", "process-releases"):
        passed.append(checks_mon[2])
    else:
        failed.append(checks_mon[2])
    if _commands_contain(commands, "bacchus", "stale"):
        passed.append(checks_mon[3])
    else:
        failed.append(checks_mon[3])
    if _commands_contain(commands, "bacchus", "events"):
        passed.append(checks_mon[4])
    else:
        failed.append(checks_mon[4])
    if _commands_contain(commands, "bacchus", "message", "list"):
        passed.append(checks_mon[5])
    else:
        failed.append(checks_mon[5])
    if _commands_contain(commands, "bacchus", "task", "list", "--ready"):
        passed.append(checks_mon[6])
    else:
        failed.append(checks_mon[6])
    # Polling cadence
    if any(kw in transcript_text.lower() for kw in ["sleep", "wait", "30s", "30 s", "polling"]):
        passed.append(checks_mon[7])
    else:
        failed.append(checks_mon[7])
    cat_score = len(passed) / max(len(checks_mon), 1)
    details["monitor_loop"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_mon)} monitor checks passed",
    }

    # --- recovery ---
    checks_rec = ORCHESTRATOR_RUBRIC["recovery"]["checks"]
    passed, failed = [], []
    # Only score recovery checks that are relevant to the scenario
    if rubric_checks.get("runs_stale_cleanup") or rubric_checks.get("runs_monitor_loop"):
        if _commands_contain(commands, "stale", "--cleanup"):
            passed.append(checks_rec[0])
        else:
            failed.append(checks_rec[0])
    else:
        passed.append(checks_rec[0])
    if rubric_checks.get("allows_retry") or rubric_checks.get("spawns_replacement_worker"):
        if spawn_count >= 2 or "retry" in transcript_text.lower() or "reset" in transcript_text.lower():
            passed.append(checks_rec[1])
        else:
            failed.append(checks_rec[1])
    else:
        passed.append(checks_rec[1])
    if rubric_checks.get("stops_retrying_after_3") or rubric_checks.get("triggers_replan"):
        if any(kw in transcript_text.lower() for kw in ["re-plan", "replan", "3 times", "three times", "stop retry"]):
            passed.append(checks_rec[2])
        else:
            failed.append(checks_rec[2])
    else:
        passed.append(checks_rec[2])
    if rubric_checks.get("detects_merge_conflict") or rubric_checks.get("messages_responsible_agent"):
        if _commands_contain(commands, "bacchus", "message", "send"):
            passed.append(checks_rec[3])
        else:
            failed.append(checks_rec[3])
    else:
        passed.append(checks_rec[3])
    if rubric_checks.get("checks_messages") or rubric_checks.get("adjusts_footprints_or_splits_task"):
        if _commands_contain(commands, "bacchus", "message") or "footprint" in transcript_text.lower():
            passed.append(checks_rec[4])
        else:
            failed.append(checks_rec[4])
    else:
        passed.append(checks_rec[4])
    cat_score = len(passed) / max(len(checks_rec), 1)
    details["recovery"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_rec)} recovery checks passed",
    }

    # --- release_processing ---
    checks_rel = ORCHESTRATOR_RUBRIC["release_processing"]["checks"]
    passed, failed = [], []
    if _commands_contain(commands, "bacchus", "process-releases"):
        passed.append(checks_rel[0])
    else:
        failed.append(checks_rel[0])
    if rubric_checks.get("runs_package_install") or rubric_checks.get("detects_dependency_change"):
        if _commands_contain_any(commands, "bun install", "npm install", "pnpm install", "yarn install", "cargo build"):
            passed.append(checks_rel[1])
        else:
            failed.append(checks_rel[1])
    else:
        passed.append(checks_rel[1])
    if rubric_checks.get("task_description_says_no_install"):
        if any(kw in transcript_text.lower() for kw in ["not run install", "no install", "don't install", "do not install", "must not"]):
            passed.append(checks_rel[2])
        else:
            failed.append(checks_rel[2])
    else:
        passed.append(checks_rel[2])
    cat_score = len(passed) / max(len(checks_rel), 1)
    details["release_processing"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_rel)} release checks passed",
    }

    # --- hard_rules ---
    checks_hr = ORCHESTRATOR_RUBRIC["hard_rules"]["checks"]
    passed, failed = [], []
    # Never writes source code (only tasks.yaml allowed)
    source_files = [f for f in files if f and "tasks.yaml" not in f and ".bacchus/" not in f]
    violations = transcript.get("violations", [])
    if not source_files and "writes_code" not in str(violations):
        passed.append(checks_hr[0])
    else:
        failed.append(checks_hr[0])
    # Never runs claim/next/release
    if not _commands_contain_any(commands, "bacchus claim", "bacchus next", "bacchus release"):
        passed.append(checks_hr[1])
    else:
        failed.append(checks_hr[1])
    # Never edits workspace files
    workspace_edits = [f for f in files if ".bacchus/workspaces/" in f]
    if not workspace_edits:
        passed.append(checks_hr[2])
    else:
        failed.append(checks_hr[2])
    # Only output is tasks.yaml
    non_tasks_files = [f for f in files if f and "tasks.yaml" not in f and not f.startswith(".bacchus/")]
    if not non_tasks_files:
        passed.append(checks_hr[3])
    else:
        failed.append(checks_hr[3])
    # Never uses Agent tool
    if not agent_tool_used:
        passed.append(checks_hr[4])
    else:
        failed.append(checks_hr[4])
    # Never uses git worktrees
    if not _commands_contain_any(commands, "git worktree", "EnterWorktree"):
        passed.append(checks_hr[5])
    else:
        failed.append(checks_hr[5])
    cat_score = len(passed) / max(len(checks_hr), 1)
    details["hard_rules"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_hr)} hard rule checks passed",
    }

    # --- compute weighted total ---
    total = 0.0
    for category, config in ORCHESTRATOR_RUBRIC.items():
        cat_score = details.get(category, {}).get("score", 0.0)
        total += cat_score * config["weight"]

    return total, details


def evaluate_orchestrator(candidate_prompt: str, example: dict) -> tuple[float, dict]:
    """Proxy evaluator for orchestrator prompt optimization.

    1. Simulate orchestrator behavior via claude -p (1 LLM call)
    2. Score the transcript deterministically via pattern matching (0 LLM calls)

    Args:
        candidate_prompt: The orchestrator prompt text being evaluated.
        example: A scenario dict from cases/orchestrator/scenarios.json.

    Returns:
        (score, details) where score is 0.0-1.0 and details is per-category.
    """
    sim_model = os.environ.get("BACCHUS_SIM_MODEL", None)

    # Step 1: Simulate (1 LLM call)
    sim_prompt = build_orchestrator_simulation_prompt(candidate_prompt, example)
    transcript = run_orchestrator_simulation(sim_prompt, model=sim_model)

    # Step 2: Score deterministically (0 LLM calls)
    score, details = score_orchestrator_deterministic(transcript, example)

    return score, details


# ---------------------------------------------------------------------------
# Planner simulation + evaluation
# ---------------------------------------------------------------------------

_PLANNER_SIMULATION_SYSTEM = """\
You are simulating a Claude Code agent that has been given a planner prompt and a goal.
The planner decomposes a goal into a .bacchus/tasks.yaml file for parallel agent execution.

Produce a JSON object with:
{
  "commands": ["bacchus index src/", "bacchus symbols --search auth", ...],
  "tasks_yaml": "<valid YAML string for the tasks file>",
  "summary": {
    "total_tasks": 5,
    "parallel_initial": 3,
    "critical_path_length": 2
  }
}

Rules:
- commands: list EXACT shell commands the agent would run (bacchus index, bacchus symbols, cat/read files, bacchus task import, validate, list --ready)
- tasks_yaml: produce VALID YAML content for the tasks file. Each task MUST have: id, title, task_type, archetype, description, depends_on, footprint (with modifies/creates).
- Descriptions should be detailed (50-500 words) with file paths, symbol names, gate criteria. Do NOT include line numbers.
- Footprints should use symbol-level refs where possible (file::Symbol) and not overlap between concurrent tasks.
- summary: include total_tasks, parallel_initial (tasks with no dependencies), critical_path_length
- Output ONLY the JSON object, no other text."""


def build_planner_simulation_prompt(candidate_prompt: str, scenario: dict) -> str:
    """Build the user message for planner simulation."""
    parts = [
        "## Planner Prompt (the agent's instructions)\n",
        candidate_prompt,
        "\n\n## Planning Scenario\n",
        f"**Goal:** {scenario['epic_goal']}\n",
        f"**Input format:** {scenario['input_format']}\n",
        f"**Expected tasks:** ~{scenario['num_expected_tasks']}\n",
        f"**Archetypes needed:** {json.dumps(scenario['archetypes_needed'])}\n",
    ]

    if scenario.get("prd"):
        parts.append(f"\n**PRD/Requirements:**\n{scenario['prd']}\n")

    if scenario.get("file_pointer"):
        parts.append(f"\n**File pointer:** {scenario['file_pointer']}\n")

    ctx = scenario.get("codebase_context", {})
    if ctx:
        parts.append(f"\n**Project:** {ctx.get('project', 'unknown')} ({ctx.get('language', 'unknown')})\n")
        if ctx.get("file_tree"):
            parts.append("\n**File tree:**\n```\n")
            for f in ctx["file_tree"]:
                parts.append(f"{f}\n")
            parts.append("```\n")
        if ctx.get("symbols"):
            parts.append("\n**Key symbols:**\n")
            for s in ctx["symbols"]:
                parts.append(f"- {s}\n")

    parts.append(
        "\n---\n"
        "Simulate this planner agent decomposing the goal into tasks. "
        "Produce the full output as JSON including commands, tasks_yaml, and summary."
    )
    return "".join(parts)


def run_planner_simulation(prompt: str, model: str | None = None) -> dict:
    """Run planner simulation via claude -p and parse the JSON transcript."""
    try:
        text = _claude_prompt(prompt, system=_PLANNER_SIMULATION_SYSTEM, model=model)
    except subprocess.TimeoutExpired:
        print("  [TIMEOUT] Planner simulation timed out", file=sys.stderr)
        return {"commands": [], "tasks_yaml": "", "summary": {}, "error": "Simulation timed out"}
    result = _parse_json(text)
    if not result:
        return {"commands": [], "tasks_yaml": "", "summary": {}, "error": "Failed to parse simulation"}
    return result


def _parse_tasks_yaml(yaml_str: str) -> list[dict]:
    """Parse a tasks_yaml string into a list of task dicts."""
    if not yaml_str or not yaml_str.strip():
        return []

    try:
        data = yaml.safe_load(yaml_str)
    except yaml.YAMLError:
        return []

    if isinstance(data, dict) and "tasks" in data:
        tasks = data["tasks"]
    elif isinstance(data, list):
        tasks = data
    else:
        return []

    return [t for t in tasks if isinstance(t, dict)]


def _count_words(text: str) -> int:
    """Count words in a text string."""
    return len(text.split()) if text else 0


def _has_cycle(tasks: list[dict]) -> bool:
    """Check if the dependency graph has a cycle (DFS)."""
    task_ids = {t.get("id", "") for t in tasks}
    deps: dict[str, list[str]] = {}
    for t in tasks:
        tid = t.get("id", "")
        deps[tid] = [d for d in (t.get("depends_on") or []) if d in task_ids]

    WHITE, GRAY, BLACK = 0, 1, 2
    color = {tid: WHITE for tid in task_ids}

    def dfs(node: str) -> bool:
        color[node] = GRAY
        for dep in deps.get(node, []):
            if color.get(dep) == GRAY:
                return True
            if color.get(dep) == WHITE and dfs(dep):
                return True
        color[node] = BLACK
        return False

    return any(color[tid] == WHITE and dfs(tid) for tid in task_ids)


def _get_concurrent_pairs(tasks: list[dict]) -> list[tuple[dict, dict]]:
    """Get all pairs of tasks that could run concurrently (no dependency relationship)."""
    task_map = {t.get("id", ""): t for t in tasks}

    # Build full reachability (transitive closure of depends_on)
    all_deps: dict[str, set[str]] = {}
    for t in tasks:
        tid = t.get("id", "")
        all_deps[tid] = set(t.get("depends_on") or [])

    # Transitive closure
    changed = True
    while changed:
        changed = False
        for tid, deps in all_deps.items():
            new_deps = set()
            for d in deps:
                new_deps |= all_deps.get(d, set())
            before = len(deps)
            deps |= new_deps
            if len(deps) > before:
                changed = True

    pairs = []
    task_list = list(task_map.keys())
    for i in range(len(task_list)):
        for j in range(i + 1, len(task_list)):
            a, b = task_list[i], task_list[j]
            # Concurrent if neither depends on the other (transitively)
            if a not in all_deps.get(b, set()) and b not in all_deps.get(a, set()):
                pairs.append((task_map[a], task_map[b]))

    return pairs


def _get_footprint_entries(task: dict) -> set[str]:
    """Extract all footprint entries (modifies + creates) from a task."""
    fp = task.get("footprint") or {}
    entries = set()
    for entry in (fp.get("modifies") or []):
        entries.add(entry)
    for entry in (fp.get("creates") or []):
        entries.add(entry)
    return entries


def _footprints_overlap(fp_a: set[str], fp_b: set[str]) -> bool:
    """Check if two footprint entry sets overlap (file-level comparison)."""
    # Normalize to file level for overlap check (strip ::Symbol suffix)
    def file_of(entry: str) -> str:
        return entry.split("::")[0] if "::" in entry else entry

    files_a = {file_of(e) for e in fp_a}
    files_b = {file_of(e) for e in fp_b}
    return bool(files_a & files_b)


def score_planner_deterministic(
    transcript: dict,
    scenario: dict,
) -> tuple[float, dict]:
    """Score a planner transcript deterministically via YAML parsing + pattern matching.

    Returns (weighted_score, category_details).
    """
    commands = _get_commands(transcript)
    tasks_yaml_str = transcript.get("tasks_yaml", "")
    tasks = _parse_tasks_yaml(tasks_yaml_str)
    summary = transcript.get("summary", {})
    rubric_checks = scenario.get("rubric", {})

    details: dict[str, dict] = {}

    # --- codebase_exploration ---
    checks_exp = PLANNER_RUBRIC["codebase_exploration"]["checks"]
    passed, failed = [], []
    if _commands_contain(commands, "bacchus", "index"):
        passed.append(checks_exp[0])
    else:
        failed.append(checks_exp[0])
    if _commands_contain(commands, "bacchus", "symbols", "--search"):
        passed.append(checks_exp[1])
    else:
        failed.append(checks_exp[1])
    # Reads files — check for cat, read, or file read commands
    if _commands_contain_any(commands, "cat ", "read ", "bacchus handle expand", "bacchus symbols"):
        passed.append(checks_exp[2])
    else:
        failed.append(checks_exp[2])
    cat_score = len(passed) / max(len(checks_exp), 1)
    details["codebase_exploration"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_exp)} exploration checks passed",
    }

    # --- task_decomposition ---
    checks_decomp = PLANNER_RUBRIC["task_decomposition"]["checks"]
    passed, failed = [], []
    target = rubric_checks.get("task_count_target", scenario.get("num_expected_tasks", 3))
    actual = len(tasks)
    # Within ±50%
    low = max(1, int(target * 0.5))
    high = int(target * 1.5)
    if low <= actual <= high:
        passed.append(checks_decomp[0])
    else:
        failed.append(checks_decomp[0])
    # Required fields
    required_fields = ["id", "title", "task_type", "archetype", "depends_on", "footprint"]
    if tasks:
        all_have_fields = all(
            all(f in t for f in required_fields)
            for t in tasks
        )
        if all_have_fields:
            passed.append(checks_decomp[1])
        else:
            failed.append(checks_decomp[1])
    else:
        failed.append(checks_decomp[1])
    # Description word count 50-500
    if tasks:
        desc_sizes = [_count_words(t.get("description", "")) for t in tasks]
        good_sizes = sum(1 for w in desc_sizes if 50 <= w <= 500)
        if good_sizes >= len(tasks) * 0.7:  # 70% threshold
            passed.append(checks_decomp[2])
        else:
            failed.append(checks_decomp[2])
    else:
        failed.append(checks_decomp[2])
    cat_score = len(passed) / max(len(checks_decomp), 1)
    details["task_decomposition"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_decomp)} decomposition checks passed (actual={actual}, target={target})",
    }

    # --- footprint_quality ---
    checks_fp = PLANNER_RUBRIC["footprint_quality"]["checks"]
    passed, failed = [], []
    if tasks:
        # Symbol-level preference: count entries with :: vs total
        all_entries = []
        for t in tasks:
            all_entries.extend(_get_footprint_entries(t))
        symbol_level = sum(1 for e in all_entries if "::" in e)
        total_entries = len(all_entries)
        if total_entries > 0 and symbol_level / total_entries >= 0.3:
            passed.append(checks_fp[0])
        else:
            failed.append(checks_fp[0])

        # No overlap between concurrent tasks
        concurrent_pairs = _get_concurrent_pairs(tasks)
        has_overlap = False
        fp_cache = {t.get("id", ""): _get_footprint_entries(t) for t in tasks}
        for ta, tb in concurrent_pairs:
            if _footprints_overlap(fp_cache[ta.get("id", "")], fp_cache[tb.get("id", "")]):
                has_overlap = True
                break
        if not has_overlap:
            passed.append(checks_fp[1])
        else:
            failed.append(checks_fp[1])

        # Non-empty footprints
        all_have_fp = all(len(_get_footprint_entries(t)) > 0 for t in tasks)
        if all_have_fp:
            passed.append(checks_fp[2])
        else:
            failed.append(checks_fp[2])
    else:
        failed.extend(checks_fp)
    cat_score = len(passed) / max(len(checks_fp), 1)
    details["footprint_quality"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_fp)} footprint checks passed",
    }

    # --- description_quality ---
    checks_desc = PLANNER_RUBRIC["description_quality"]["checks"]
    passed, failed = [], []
    if tasks:
        descriptions = [t.get("description", "") for t in tasks]
        all_descs = " ".join(descriptions)

        # File path references
        has_file_paths = any(p.search(all_descs) for p in _RE_FILE_PATHS)
        if has_file_paths:
            passed.append(checks_desc[0])
        else:
            failed.append(checks_desc[0])

        # Symbol name references (CamelCase or snake_case identifiers that look like code)
        has_symbols = rubric_checks.get("expects_symbol_refs", True) is False or any(
            p.search(all_descs) for p in _RE_SYMBOLS
        )
        if has_symbols:
            passed.append(checks_desc[1])
        else:
            failed.append(checks_desc[1])

        # NO line numbers (penalize)
        has_line_numbers = any(p.search(all_descs) for p in _RE_LINE_NUMBERS)
        if not has_line_numbers:
            passed.append(checks_desc[2])
        else:
            failed.append(checks_desc[2])

        # Gate criteria
        has_gates = any(p.search(all_descs) for p in _RE_GATES)
        if has_gates:
            passed.append(checks_desc[3])
        else:
            failed.append(checks_desc[3])

        # Pattern refs or self-contained
        has_pattern_refs = any(p.search(all_descs) for p in _RE_PATTERNS)
        # Self-contained = descriptions are long enough to stand alone
        avg_words = sum(_count_words(d) for d in descriptions) / max(len(descriptions), 1)
        if has_pattern_refs or avg_words >= 80:
            passed.append(checks_desc[4])
        else:
            failed.append(checks_desc[4])

        # No-install notes (only when scenario requires it)
        if rubric_checks.get("expects_no_install_note"):
            has_no_install = any(p.search(all_descs) for p in _RE_NO_INSTALL)
            if has_no_install:
                passed.append(checks_desc[5])
            else:
                failed.append(checks_desc[5])
        else:
            passed.append(checks_desc[5])  # N/A
    else:
        failed.extend(checks_desc)
    cat_score = len(passed) / max(len(checks_desc), 1)
    details["description_quality"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_desc)} description checks passed",
    }

    # --- dependency_graph ---
    checks_dep = PLANNER_RUBRIC["dependency_graph"]["checks"]
    passed, failed = [], []
    if tasks:
        task_ids = {t.get("id", "") for t in tasks}

        # Valid refs
        all_deps_valid = True
        for t in tasks:
            for dep in (t.get("depends_on") or []):
                if dep not in task_ids:
                    all_deps_valid = False
                    break
        if all_deps_valid:
            passed.append(checks_dep[0])
        else:
            failed.append(checks_dep[0])

        # No cycles
        if not _has_cycle(tasks):
            passed.append(checks_dep[1])
        else:
            failed.append(checks_dep[1])

        # Parallelism ratio > 50%
        no_deps = sum(1 for t in tasks if not (t.get("depends_on") or []))
        parallelism_ratio = no_deps / len(tasks) if tasks else 0
        if parallelism_ratio > 0.5 or not rubric_checks.get("expects_high_parallelism", True):
            passed.append(checks_dep[2])
        else:
            failed.append(checks_dep[2])

        # Minimal deps — average deps per task should be low
        total_deps = sum(len(t.get("depends_on") or []) for t in tasks)
        avg_deps = total_deps / len(tasks) if tasks else 0
        if avg_deps <= 1.5:
            passed.append(checks_dep[3])
        else:
            failed.append(checks_dep[3])
    else:
        failed.extend(checks_dep)
    cat_score = len(passed) / max(len(checks_dep), 1)
    details["dependency_graph"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_dep)} dependency checks passed",
    }

    # --- validation ---
    checks_val = PLANNER_RUBRIC["validation"]["checks"]
    passed, failed = [], []
    if _commands_contain(commands, "bacchus", "task", "import"):
        passed.append(checks_val[0])
    else:
        failed.append(checks_val[0])
    if _commands_contain(commands, "bacchus", "task", "validate"):
        passed.append(checks_val[1])
    else:
        failed.append(checks_val[1])
    if _commands_contain(commands, "bacchus", "task", "list", "--ready"):
        passed.append(checks_val[2])
    else:
        failed.append(checks_val[2])
    cat_score = len(passed) / max(len(checks_val), 1)
    details["validation"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_val)} validation checks passed",
    }

    # --- constraints ---
    checks_con = PLANNER_RUBRIC["constraints"]["checks"]
    passed, failed = [], []
    # Only writes tasks.yaml
    files = _files_written(transcript)
    source_files = [f for f in files if f and "tasks.yaml" not in f and ".bacchus/" not in f]
    if not source_files:
        passed.append(checks_con[0])
    else:
        failed.append(checks_con[0])
    # Produces summary
    has_summary = bool(summary) or "summary" in transcript
    if has_summary:
        passed.append(checks_con[1])
    else:
        failed.append(checks_con[1])
    cat_score = len(passed) / max(len(checks_con), 1)
    details["constraints"] = {
        "score": cat_score,
        "passed_checks": passed,
        "failed_checks": failed,
        "reason": f"{len(passed)}/{len(checks_con)} constraint checks passed",
    }

    # --- compute weighted total ---
    total = 0.0
    for category, config in PLANNER_RUBRIC.items():
        cat_score = details.get(category, {}).get("score", 0.0)
        total += cat_score * config["weight"]

    return total, details


def evaluate_planner(candidate_prompt: str, example: dict) -> tuple[float, dict]:
    """Proxy evaluator for planner prompt optimization.

    1. Simulate planner behavior via claude -p (1 LLM call)
    2. Score the transcript deterministically via YAML parsing + pattern matching (0 LLM calls)

    Args:
        candidate_prompt: The planner prompt text being evaluated.
        example: A scenario dict from cases/planner/scenarios.json.

    Returns:
        (score, details) where score is 0.0-1.0 and details is per-category.
    """
    sim_model = os.environ.get("BACCHUS_SIM_MODEL", None)

    # Step 1: Simulate (1 LLM call)
    sim_prompt = build_planner_simulation_prompt(candidate_prompt, example)
    transcript = run_planner_simulation(sim_prompt, model=sim_model)

    # Step 2: Score deterministically (0 LLM calls)
    score, details = score_planner_deterministic(transcript, example)

    return score, details
