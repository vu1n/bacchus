#!/usr/bin/env python3
"""Apply an optimized prompt back to skills/ and show the diff.

Usage:
    uv run python apply.py --prompt worker                    # latest optimized
    uv run python apply.py --prompt worker --file optimized/worker_20260307_120000.md
    uv run python apply.py --prompt worker --dry-run          # show diff only
"""

from __future__ import annotations

import argparse
import difflib
import shutil
import sys
from pathlib import Path

PROMPT_PATHS: dict[str, str] = {
    "worker": "skills/bacchus/commands/bacchus-worker.md",
    "orchestrator": "skills/bacchus/commands/bacchus-orchestrator.md",
    "planner": "skills/bacchus/commands/bacchus-plan.md",
}

REPO_ROOT = Path(__file__).resolve().parent.parent
OPTIMIZED_DIR = Path(__file__).parent / "optimized"


def find_latest_optimized(prompt_type: str) -> Path | None:
    """Find the most recent optimized prompt file for a given type."""
    pattern = f"{prompt_type}_*.md"
    candidates = sorted(
        OPTIMIZED_DIR.glob(pattern),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )
    # Filter out metadata files
    candidates = [c for c in candidates if not c.name.endswith("_meta.json")]
    return candidates[0] if candidates else None


def show_diff(original: str, optimized: str, target_path: str) -> str:
    """Generate a unified diff between original and optimized prompts."""
    original_lines = original.splitlines(keepends=True)
    optimized_lines = optimized.splitlines(keepends=True)
    diff = difflib.unified_diff(
        original_lines,
        optimized_lines,
        fromfile=f"a/{target_path}",
        tofile=f"b/{target_path}",
        n=3,
    )
    return "".join(diff)


def main():
    parser = argparse.ArgumentParser(
        description="Apply optimized prompt to skills/"
    )
    parser.add_argument(
        "--prompt",
        choices=list(PROMPT_PATHS.keys()),
        required=True,
        help="Which prompt type to apply",
    )
    parser.add_argument(
        "--file",
        type=str,
        default=None,
        help="Specific optimized file to apply (default: latest)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show diff only, don't copy",
    )
    args = parser.parse_args()

    # Find optimized file
    if args.file:
        optimized_path = Path(args.file)
        if not optimized_path.is_absolute():
            optimized_path = Path(__file__).parent / optimized_path
    else:
        optimized_path = find_latest_optimized(args.prompt)

    if optimized_path is None or not optimized_path.exists():
        print(f"No optimized prompt found for '{args.prompt}'.")
        print(f"Run: uv run python optimize.py --prompt {args.prompt}")
        sys.exit(1)

    # Load both versions
    target_rel = PROMPT_PATHS[args.prompt]
    target_path = REPO_ROOT / target_rel
    original = target_path.read_text()
    optimized = optimized_path.read_text()

    if original == optimized:
        print("No changes — optimized prompt is identical to current.")
        sys.exit(0)

    # Show diff
    diff = show_diff(original, optimized, target_rel)
    print(diff)

    # Stats
    orig_lines = len(original.splitlines())
    opt_lines = len(optimized.splitlines())
    print(f"\nOriginal: {orig_lines} lines, {len(original)} chars")
    print(f"Optimized: {opt_lines} lines, {len(optimized)} chars")
    print(f"Delta: {opt_lines - orig_lines:+d} lines, {len(optimized) - len(original):+d} chars")
    print(f"\nSource: {optimized_path}")
    print(f"Target: {target_path}")

    if args.dry_run:
        print("\n(dry run — no files modified)")
        return

    # Confirm
    response = input("\nApply this change? [y/N] ").strip().lower()
    if response != "y":
        print("Aborted.")
        sys.exit(0)

    # Apply
    shutil.copy2(optimized_path, target_path)
    print(f"\nApplied. Rebuild with: cargo build --release && cp target/release/bacchus ~/.local/bin/")


if __name__ == "__main__":
    main()
