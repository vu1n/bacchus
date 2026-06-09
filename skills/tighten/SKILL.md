---
name: tighten
description: Per-iteration code-tightening pass optimized for an AI-maintained codebase. Strips slop (deslop), applies local simplifications (simplify), then invests context the next agent needs (contextualize). Use after a round of iteration to keep the working tree tight before continuing. Invoke for "tighten", "tighten this up", "clean pass".
disable-model-invocation: true
---

# Tighten

Fast, mutating cleanup pass to run after a round of iteration so you are not building on slop. Cheap and local — no bug hunting, no ambitious restructuring. Those belong to `ship-review`.

This codebase is primarily read and written by AI agents, so the goal is not human visual polish — it is **low-friction context for the next agent**: high signal-per-token, machine-checkable guardrails, and intent that survives a cold session. That means both *removing* noise and *adding* the things an agent needs.

Three stages on the **quality axis only**. All mutate the working tree.

## Workflow

1. **deslop (subtractive)** — Invoke the `deslop` skill. Strip noise that costs tokens without adding signal: what-comments, `any`-casts, silent catch-and-ignore, gratuitous nesting, dead code. Protects why-comments, explicit types, and loud guards.
2. **simplify (local cleanup)** — Invoke the `/simplify` skill. Apply local reuse/dedup/efficiency/altitude cleanups on the now-deslopped diff.
3. **contextualize (additive)** — Invest the context the next agent needs, only where it is genuinely missing:
   - Add a **why-comment** where intent or a non-obvious constraint isn't recoverable from the code alone. Skip the obvious.
   - **Tighten loose types** — replace a surviving `any`/`unknown`/over-wide type with the real shape; make a boundary contract explicit.
   - Convert a **silent fallback into a loud guard** (`assert`/validation) where a violated invariant should fail fast rather than limp on.
   - Drop a one-line **breadcrumb at a module boundary** when behavior isn't explained end-to-end locally.

Run in order: deslop removes noise so simplify reasons over cleaner code, and contextualize adds signal last so it isn't immediately stripped.

## Scope

- Operate on the current branch diff vs base, plus uncommitted changes.
- Keep behavior unchanged (loud guards may *surface* a latent bug — that is acceptable and worth flagging).
- Prefer minimal, focused edits over broad rewrites. Additions are cheap context, not documentation theater — if a comment restates the code, it is slop, not context.

## Boundaries

- Do **not** hunt for bugs — that is `/code-review`'s job, run pre-ship via `ship-review`.
- Do **not** make ambitious structural/architectural restructures — that is `thermo-nuclear-code-quality-review`'s job, also pre-ship.
- If you notice a real bug or a large structural smell, **note it for `ship-review`** rather than acting on it here.

## Output

- One concise summary (1-3 sentences): what was stripped, simplified, and what context was added.
- A short list of anything deferred to `ship-review` (bugs spotted, structural smells, invariants a new guard surfaced).
