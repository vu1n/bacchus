#!/usr/bin/env python3
"""Optimize bacchus prompts using GEPA.

Uses a custom GEPAAdapter that shells out to `claude -p` for both
simulation and scoring, leveraging the user's Claude subscription.

Usage:
    uv run python optimize.py --prompt worker
    uv run python optimize.py --prompt worker --max-iterations 5
    uv run python optimize.py --prompt worker --eval-only
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Mapping, Sequence
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from dotenv import load_dotenv

load_dotenv()

from gepa import EvaluationBatch, GEPAResult
from gepa.api import optimize

from evaluators.proxy import (
    _claude_prompt,
    build_orchestrator_simulation_prompt,
    build_simulation_prompt,
    evaluate_orchestrator,
    evaluate_worker,
    run_orchestrator_simulation,
    run_simulation,
    score_transcript,
)
from evaluators.rubrics import ORCHESTRATOR_RUBRIC, WORKER_RUBRIC

PROMPT_PATHS: dict[str, str] = {
    "worker": "skills/bacchus/commands/bacchus-worker.md",
    "orchestrator": "skills/bacchus/commands/bacchus-orchestrator.md",
    "planner": "skills/bacchus/commands/bacchus-plan.md",
}

REPO_ROOT = Path(__file__).resolve().parent.parent
OPTIMIZED_DIR = Path(__file__).parent / "optimized"


def load_seed_prompt(prompt_type: str) -> str:
    path = REPO_ROOT / PROMPT_PATHS[prompt_type]
    return path.read_text()


def load_cases(prompt_type: str) -> list[dict]:
    cases_path = Path(__file__).parent / "cases" / prompt_type / "scenarios.json"
    return json.loads(cases_path.read_text())


def split_cases(
    cases: list[dict], ratio: float = 0.8
) -> tuple[list[dict], list[dict]]:
    split_idx = max(1, int(len(cases) * ratio))
    return cases[:split_idx], cases[split_idx:]


# ---------------------------------------------------------------------------
# Custom GEPA Adapter: claude -p based simulation + scoring
# ---------------------------------------------------------------------------

# Trajectory = simulation transcript + scoring details
type Trajectory = dict[str, Any]
# RolloutOutput = (score, details)
type RolloutOutput = tuple[float, dict]


EVALUATORS = {
    "worker": evaluate_worker,
    "orchestrator": evaluate_orchestrator,
}

SIMULATION_BUILDERS = {
    "worker": (build_simulation_prompt, run_simulation),
    "orchestrator": (build_orchestrator_simulation_prompt, run_orchestrator_simulation),
}


class BacchusPromptAdapter:
    """GEPA adapter that evaluates bacchus prompts via claude -p simulation."""

    def __init__(self, prompt_type: str = "worker"):
        self.prompt_type = prompt_type
        self._evaluator = EVALUATORS[prompt_type]
        self._build_sim, self._run_sim = SIMULATION_BUILDERS[prompt_type]

    def evaluate(
        self,
        batch: list[dict],
        candidate: dict[str, str],
        capture_traces: bool = False,
    ) -> EvaluationBatch:
        prompt_text = candidate["prompt"]
        scores: list[float] = []
        outputs: list[RolloutOutput] = []
        trajectories: list[Trajectory] | None = [] if capture_traces else None

        for scenario in batch:
            try:
                score, details = self._evaluator(prompt_text, scenario)
            except Exception as e:
                print(f"  [ERROR] {scenario.get('id', '?')}: {e}", file=sys.stderr)
                score, details = 0.0, {"error": str(e)}

            scores.append(score)
            outputs.append((score, details))

            if capture_traces:
                sim_prompt = self._build_sim(prompt_text, scenario)
                transcript = self._run_sim(sim_prompt)
                trajectories.append({
                    "scenario_id": scenario.get("id", "unknown"),
                    "scenario": scenario,
                    "transcript": transcript,
                    "score": score,
                    "details": details,
                })

        return EvaluationBatch(
            outputs=outputs,
            scores=scores,
            trajectories=trajectories,
        )

    def make_reflective_dataset(
        self,
        candidate: dict[str, str],
        eval_batch: EvaluationBatch,
        components_to_update: list[str],
    ) -> Mapping[str, Sequence[Mapping[str, Any]]]:
        """Build reflection examples from failed/weak evaluations."""
        records: list[dict[str, Any]] = []

        if eval_batch.trajectories is None:
            return {"prompt": records}

        for traj in eval_batch.trajectories:
            score = traj["score"]
            details = traj["details"]
            scenario = traj["scenario"]

            # Build feedback from failed rubric categories
            feedback_parts = []
            for cat, info in details.items():
                if isinstance(info, dict):
                    cat_score = info.get("score", 0.0)
                    if cat_score < 1.0:
                        failed = info.get("failed_checks", [])
                        reason = info.get("reason", "")
                        feedback_parts.append(
                            f"[{cat}] score={cat_score:.2f}: {reason}"
                        )
                        for fc in failed:
                            feedback_parts.append(f"  - FAILED: {fc}")

            records.append({
                "Inputs": {
                    "scenario_id": scenario.get("id", "unknown"),
                    "task_type": scenario.get("task_type", "unknown"),
                    "archetype": scenario.get("archetype", "unknown"),
                    "special_conditions": scenario.get("special_conditions", "none"),
                },
                "Generated Outputs": {
                    "simulated_transcript": json.dumps(
                        traj.get("transcript", {}), indent=2
                    )[:2000],  # Truncate for token budget
                    "release_status": traj.get("transcript", {}).get(
                        "release_status", "unknown"
                    ),
                },
                "Feedback": "\n".join(feedback_parts) if feedback_parts else "All checks passed.",
                "score": score,
            })

        return {"prompt": records}

    @staticmethod
    def propose_new_texts(
        candidate: dict[str, str],
        reflective_dataset: Mapping[str, Sequence[Mapping[str, Any]]],
        components_to_update: list[str],
    ) -> dict[str, str]:
        """Propose improved prompt text using claude -p."""
        current_prompt = candidate["prompt"]
        records = reflective_dataset.get("prompt", [])

        # Build a reflection prompt for claude -p
        feedback_summary = []
        for r in records:
            inputs = r.get("Inputs", {})
            feedback = r.get("Feedback", "")
            score = r.get("score", 0.0)
            feedback_summary.append(
                f"Scenario: {inputs.get('scenario_id', '?')} "
                f"({inputs.get('task_type', '?')}/{inputs.get('archetype', '?')}) "
                f"score={score:.2f}\n"
                f"Special: {inputs.get('special_conditions', 'none')}\n"
                f"Feedback:\n{feedback}\n"
            )

        reflection_prompt = f"""\
<instructions>
You are an expert prompt engineer optimizing a worker agent prompt for a multi-agent coordination system called Bacchus.

Your task: given the current prompt and evaluation feedback, produce an IMPROVED version of the prompt.

Rules:
- The improved prompt must be a complete, standalone markdown document (not a diff)
- Focus on fixing the specific failures identified in the feedback
- Do NOT change the overall structure drastically — make targeted improvements
- Do NOT remove any existing correct behaviors
- Keep the prompt concise — don't bloat it with redundant instructions
- Output ONLY the improved prompt text, nothing else (no preamble, no explanation)
</instructions>

## Current Prompt

{current_prompt}

## Evaluation Feedback (from simulated agent runs)

{"".join(feedback_summary)}

## Task

Produce an improved version of the prompt above that addresses the failures.
Output ONLY the improved prompt markdown, starting with the first heading."""

        improved = _claude_prompt(reflection_prompt)

        # Strip any leading/trailing whitespace or markdown fences
        improved = improved.strip()
        if improved.startswith("```"):
            lines = improved.split("\n")
            improved = "\n".join(lines[1:])
            if improved.endswith("```"):
                improved = improved[:-3].rstrip()

        return {"prompt": improved}


# ---------------------------------------------------------------------------
# Save results
# ---------------------------------------------------------------------------


def save_optimized(prompt_type: str, result: GEPAResult) -> Path:
    OPTIMIZED_DIR.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
    out_path = OPTIMIZED_DIR / f"{prompt_type}_{timestamp}.md"

    # Best candidate is the one with highest val score
    best = result.best_candidate
    optimized_text = best["prompt"]
    out_path.write_text(optimized_text)

    # Save metadata
    meta_path = OPTIMIZED_DIR / f"{prompt_type}_{timestamp}_meta.json"
    meta = {
        "prompt_type": prompt_type,
        "timestamp": timestamp,
        "best_val_idx": result.best_idx,
        "num_candidates": result.num_candidates,
        "total_metric_calls": result.total_metric_calls,
    }
    meta_path.write_text(json.dumps(meta, indent=2))

    return out_path


# ---------------------------------------------------------------------------
# Runners
# ---------------------------------------------------------------------------


def run_eval_only(prompt_type: str, max_cases: int | None = None):
    from evaluate import run_evaluation

    prompt = load_seed_prompt(prompt_type)
    cases = load_cases(prompt_type)
    if max_cases:
        cases = cases[:max_cases]

    evaluator = EVALUATORS[prompt_type]
    summary = run_evaluation(prompt, evaluator, cases)

    OPTIMIZED_DIR.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
    out_path = OPTIMIZED_DIR / f"{prompt_type}_baseline_{timestamp}.json"
    out_path.write_text(json.dumps(summary, indent=2))
    print(f"\nBaseline results saved to {out_path}")


def run_optimization(
    prompt_type: str,
    max_iterations: int = 30,
    max_cases: int | None = None,
):
    seed = load_seed_prompt(prompt_type)
    cases = load_cases(prompt_type)
    if max_cases:
        cases = cases[:max_cases]

    train, val = split_cases(cases)
    adapter = BacchusPromptAdapter(prompt_type)

    seed_candidate = {"prompt": seed}

    print(f"Optimizing '{prompt_type}' prompt")
    print(f"  Seed length: {len(seed)} chars")
    print(f"  Train cases: {len(train)}, Val cases: {len(val)}")
    print(f"  Max metric calls: {max_iterations * len(train)}")
    print(f"  Reflection via: claude -p")
    print()

    result = optimize(
        seed_candidate=seed_candidate,
        trainset=train,
        valset=val,
        adapter=adapter,
        max_metric_calls=max_iterations * len(train),
        reflection_minibatch_size=min(3, len(train)),
        display_progress_bar=True,
    )

    out_path = save_optimized(prompt_type, result)
    print(f"\nOptimized prompt saved to {out_path}")
    print(f"Best candidate idx: {result.best_idx}")
    print(f"Total metric calls: {result.total_metric_calls}")
    print(f"Candidates evaluated: {result.num_candidates}")


def main():
    parser = argparse.ArgumentParser(
        description="Optimize bacchus prompts with GEPA"
    )
    parser.add_argument(
        "--prompt",
        choices=list(PROMPT_PATHS.keys()),
        required=True,
    )
    parser.add_argument(
        "--eval-only",
        action="store_true",
    )
    parser.add_argument(
        "--max-iterations",
        type=int,
        default=30,
    )
    parser.add_argument(
        "--max-cases",
        type=int,
        default=None,
    )
    args = parser.parse_args()

    if args.eval_only:
        run_eval_only(args.prompt, args.max_cases)
    else:
        run_optimization(args.prompt, args.max_iterations, args.max_cases)


if __name__ == "__main__":
    main()
