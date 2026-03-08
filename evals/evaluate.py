#!/usr/bin/env python3
"""Standalone eval runner — score a bacchus prompt against all scenarios.

Usage:
    uv run python evaluate.py --prompt worker
    uv run python evaluate.py --prompt worker --cases 3  # first N cases only
    uv run python evaluate.py --prompt worker --case-id basic-backend-feature
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from dotenv import load_dotenv

load_dotenv()

# Prompt type -> seed file path (relative to repo root)
PROMPT_PATHS: dict[str, str] = {
    "worker": "skills/bacchus/commands/bacchus-worker.md",
    "orchestrator": "skills/bacchus/commands/bacchus-orchestrator.md",
    "planner": "skills/bacchus/commands/bacchus-plan.md",
}

REPO_ROOT = Path(__file__).resolve().parent.parent


def load_seed_prompt(prompt_type: str) -> str:
    """Load the current prompt from skills/."""
    path = REPO_ROOT / PROMPT_PATHS[prompt_type]
    return path.read_text()


def load_cases(prompt_type: str) -> list[dict]:
    """Load eval cases for a prompt type."""
    cases_path = Path(__file__).parent / "cases" / prompt_type / "scenarios.json"
    return json.loads(cases_path.read_text())


def run_evaluation(
    prompt: str,
    evaluator,
    cases: list[dict],
    verbose: bool = True,
) -> dict:
    """Score a prompt against all cases. Returns aggregate results."""
    results = []
    total_score = 0.0

    for i, case in enumerate(cases):
        case_id = case.get("id", f"case-{i}")
        if verbose:
            print(f"\n[{i + 1}/{len(cases)}] Evaluating: {case_id}", flush=True)

        score, details = evaluator(prompt, case)
        total_score += score
        results.append({"case_id": case_id, "score": score, "details": details})

        if verbose:
            print(f"  Score: {score:.3f}")
            for cat, info in details.items():
                cat_score = info.get("score", 0.0)
                marker = "PASS" if cat_score >= 0.8 else "FAIL"
                print(f"    [{marker}] {cat}: {cat_score:.2f}")
                for fc in info.get("failed_checks", []):
                    print(f"          - {fc}")

    avg_score = total_score / len(cases) if cases else 0.0

    summary = {
        "prompt_type": "unknown",
        "num_cases": len(cases),
        "avg_score": avg_score,
        "min_score": min(r["score"] for r in results) if results else 0.0,
        "max_score": max(r["score"] for r in results) if results else 0.0,
        "results": results,
    }

    if verbose:
        print(f"\n{'=' * 60}")
        print(f"BASELINE SCORE: {avg_score:.3f} (min={summary['min_score']:.3f}, max={summary['max_score']:.3f})")
        print(f"Cases evaluated: {len(cases)}")
        print(f"{'=' * 60}")

    return summary


def main():
    parser = argparse.ArgumentParser(description="Evaluate a bacchus prompt")
    parser.add_argument(
        "--prompt",
        choices=list(PROMPT_PATHS.keys()),
        required=True,
        help="Which prompt to evaluate",
    )
    parser.add_argument(
        "--cases",
        type=int,
        default=None,
        help="Limit to first N cases (for quick iteration)",
    )
    parser.add_argument(
        "--case-id",
        type=str,
        default=None,
        help="Run a single case by ID",
    )
    parser.add_argument(
        "--output",
        type=str,
        default=None,
        help="Write JSON results to file",
    )
    args = parser.parse_args()

    # Import evaluators
    from evaluators.proxy import evaluate_orchestrator, evaluate_worker

    evaluators = {"worker": evaluate_worker, "orchestrator": evaluate_orchestrator}
    evaluator = evaluators.get(args.prompt)
    if evaluator is None:
        print(f"No evaluator for '{args.prompt}' yet. Available: {list(evaluators.keys())}")
        sys.exit(1)

    # Load prompt and cases
    prompt = load_seed_prompt(args.prompt)
    cases = load_cases(args.prompt)

    # Filter cases
    if args.case_id:
        cases = [c for c in cases if c["id"] == args.case_id]
        if not cases:
            print(f"Case '{args.case_id}' not found.")
            sys.exit(1)
    elif args.cases:
        cases = cases[: args.cases]

    print(f"Evaluating '{args.prompt}' prompt against {len(cases)} cases...")
    summary = run_evaluation(prompt, evaluator, cases)

    if args.output:
        Path(args.output).write_text(json.dumps(summary, indent=2))
        print(f"\nResults written to {args.output}")


if __name__ == "__main__":
    main()
