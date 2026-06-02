---
name: orchestrator-parallelization
description: When to fan out dispatches in parallel vs sequentially. The default is sequential. Parallelize only when different file surface, independent semantics, and predictable per-task pattern all hold. Different repos is the canonical parallel case; same-repo parallelism needs worktrees.
---

# Parallelization heuristics

**The default is sequential.** Parallelize when ALL of these hold:

1. **Different file surface.** Tasks A and B touch disjoint files.
   Same-file parallel = merge-conflict hell.
2. **Independent semantics.** Task B's prompt doesn't reference the
   merged result of A.
3. **Predictable per-task pattern.** First-run of a new dispatch
   shape goes serial; parallel obscures which dispatch produced
   which lesson.

**Different repos** is the canonical parallel case -- zero file
overlap by construction.

## How to parallelize

- Each dispatch runs in its own branch + draft PR + watch loop. The
  lifecycle is already parallel-safe.
- **For same-repo parallelism:** use `roba -w=<branch-name> --fresh
  ...` so each run gets its own git worktree. The worktree IS the
  isolation; without it, concurrent runs clobber the working tree.
- Multiple `run_in_background=true` Bash calls fire roba
  simultaneously.
- **Cap concurrency at 3-5.** Beyond that, cognitive load and token
  cost outpace wall-clock savings.
- Wait for ANY notification, then handle that one PR. The harness
  notifies you per completed job.
- Merge in any order CI lands. The last few may need rebase if main
  moved -- that's the runner's problem, not yours.

## When NOT to parallelize

- Same files (sequential is faster end-to-end than rebase-conflict
  resolution).
- Hard dependency (#X blocks on #Y -- finish Y first).
- Soft dependency (B's prompt references "the result of A" -- you'd
  be writing B's prompt against a stale assumption).
- New pattern dogfooding (first run of a new dispatch shape, lean
  serial so the lesson is clean).
- Review bandwidth (if YOU can't review N PRs concurrently, fanning
  out costs you more than it saves the user).

## Cost awareness

Parallel = N× tokens per round. Honest tradeoff. Worth it when
wall-clock matters (multi-repo work where the human's blocked); not
worth it for casual backlog grinding.

## Related

- [`dispatch-via-bash`](../dispatch-via-bash/SKILL.md) -- the
  dispatch mechanism each parallel run uses.
- [`dispatch-wait-react`](../dispatch-wait-react/SKILL.md) -- how to
  coordinate the multi-dispatch wait + notification fan-in.
- [`orchestration-patterns`](../orchestration-patterns/SKILL.md) --
  which pattern you're parallelizing within (P1 same-repo
  parallel; P2 multi-repo parallel is the natural case).
