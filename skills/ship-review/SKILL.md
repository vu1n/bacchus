---
name: ship-review
description: Pre-ship deep review pass. Runs the ambitious structural quality review (thermo-nuclear, advisory) then bug review (code-review) on the settled diff, with a human checkpoint between. Use once before opening a PR. Invoke for "ship review", "deep review before PR", "review before shipping".
disable-model-invocation: true
---

# Ship Review

The heavier, pre-ship review pass. Run this once before opening a PR — not every iteration (use `tighten` for per-round cleanups).

Three lenses, in order, with a human checkpoint between structure and the rest:

1. **Structural quality** (advisory, ambitious) — what to restructure.
2. **Correctness** (review) — bugs in the code you are actually keeping.
3. **Security** (advisory) — exploitability of the settled diff.

The order matters: settle the structure first, then review the final shape for bugs and exploits, so you are not reviewing code you are about to delete.

## Workflow

### Phase 1 — Structural quality (advisory)

Invoke `thermo-nuclear-code-quality-review`. It produces ambitious, behavior-adjacent restructuring recommendations (code-judo, deleting layers, re-abstracting), prioritized by structural impact.

### Phase 1b — Refute the top structural findings

Ambitious restructure suggestions are exactly the plausible-but-wrong risk: the model proposes a "code-judo" move that doesn't actually exist or quietly changes behavior. Before presenting anything, refute the **top findings** (say, the 3-5 highest-impact, plus any whose restructure spans more than one module).

For each, spawn an independent skeptic — `subagent_type: "general-purpose"`, all in one message, **never fork**. A fork inherits the proposer's reasoning and rubber-stamps it; the point is a cold read that checks the claim against the source. Give each skeptic only the finding (location + the proposed move) and read-only repo access. Each answers:

- **Does the move exist?** Are the pieces it assumes (the helper to reuse, the seam to collapse, the branches that "disappear") actually there in the code?
- **Is it behavior-preserving?** Name one input or path where the restructured version would differ. If you can name one, the move is unsound as stated.
- **Does it actually delete complexity**, or just relocate it?

Verdict per finding: `holds` / `unsound` / `needs author input`, one line citing `file:line`. Drop or down-rank the `unsound` ones; tag the survivors with what the skeptic confirmed.

Present the surviving findings. **Do not auto-apply them** — these are behavior-adjacent and ambitious.

### Phase 2 — Checkpoint

Stop and let the user pick which restructures to take. Apply only the selected ones. If the user takes none, proceed.

If meaningful restructures were applied, the diff has changed — Phases 3 and 4 review the new shape.

### Phase 3 — Correctness (review)

Invoke `/code-review` on the settled diff. This is the bug pass: correctness, regressions, edge cases. Run it after the checkpoint so it reviews the code being kept, including anything the Phase 2 restructure introduced.

### Phase 4 — Security (advisory)

Invoke `security-pass` on the settled diff. This is the exploitability pass: recall-first finders, then fresh-context refuters, then precondition-derived severity. Keep it separate from Phase 3 — correctness asks "is this wrong?", security asks "can an attacker abuse this?". Advisory, like Phase 1: report, don't auto-fix.

## Boundaries

- Phases 1 and 4 are advisory — never apply their suggestions without the checkpoint / a separate fix task.
- Keep the lenses separate. Don't let the structural pass hunt bugs, the bug pass redesign architecture, or the security pass restructure; each is sharper focused.
- Assume `tighten` (deslop + simplify) already ran this round, so local slop and trivial cleanups are out of scope here.

## Output

- Phase 1/1b: prioritized structural findings (the code-judo move, and what the refute pass confirmed or killed).
- Phase 2: which restructures were applied.
- Phase 3: bug findings by severity (critical / warning / note).
- Phase 4: confirmed security findings (HIGH/MEDIUM/LOW) and what was dropped.
- Overall ship-readiness.
