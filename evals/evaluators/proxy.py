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

from .rubrics import WORKER_RUBRIC

CLAUDE_BIN = os.environ.get("CLAUDE_BIN", "claude")


def _claude_prompt(prompt: str, system: str | None = None, model: str | None = None) -> str:
    """Call claude -p with a prompt string, return raw text output."""
    # Combine system + user into a single prompt for claude -p
    if system:
        full_prompt = f"<instructions>\n{system}\n</instructions>\n\n{prompt}"
    else:
        full_prompt = prompt

    cmd = [CLAUDE_BIN, "-p"]
    if model:
        cmd.extend(["--model", model])

    # Strip CLAUDECODE env var to allow nested claude -p invocations
    env = {k: v for k, v in os.environ.items() if k != "CLAUDECODE"}

    result = subprocess.run(
        cmd,
        input=full_prompt,
        capture_output=True,
        text=True,
        timeout=120,
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
