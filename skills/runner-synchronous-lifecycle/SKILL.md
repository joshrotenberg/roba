---
name: runner-synchronous-lifecycle
description: The runner's invocation must hold open until the full lifecycle is done. Fire roba SYNCHRONOUSLY (no `run_in_background`); your return-to-orchestrator signals "PR pushed, CI running, lifecycle complete." Returning earlier orphans the work.
---

# Runner synchronous discipline

Your (the runner's) invocation must hold open until the full
lifecycle is done. **Returning to the orchestrator signals "task
complete: PR is pushed, CI is running (or done), ready for review."**
Returning earlier orphans the work.

## The anti-pattern this prevents

Original failure mode observed on the work machine (roba #104):

1. Runner fires roba with `run_in_background=true`
2. Runner reports a summary and returns
3. Orchestrator gets a "completed" notification for the runner
4. roba is still running locally; the commit never gets pushed; CI
   never starts; the orchestrator thinks the task is done when it
   isn't.

## Discipline that prevents it

- **Roba is fired synchronously** (no `run_in_background` on the
  roba call). Your session blocks until roba exits.

  ```bash
  roba --fresh --full-auto -C <repo-path> -f /tmp/roba-task-<N>.md
  ```

  Set a generous timeout (the harness max is 600000 ms / 10 min;
  pick what fits the task size).

- **CI watch CAN use `run_in_background=true`** because the watch is
  part of your runner's lifecycle and YOU wait for the notification
  yourself before returning. The
  [`dispatch-wait-react`](../dispatch-wait-react/SKILL.md) skill is
  the operational guide for that wait.

- **Push, mark ready, merge are all within your session.** Don't
  hand them off to "the orchestrator will pick this up." The
  orchestrator's expectation is that when your invocation returns,
  the lifecycle is done.

## What return-to-orchestrator means

When you DO return to the orchestrator:

- **Success case:** report PR number, merge commit hash, any
  caller-actionable notes (live-test follow-up, surfaced gaps in
  the issue spec, etc.).
- **Failure case:** report what failed, where (roba run? CI? push
  conflict?), the failing job's URL if applicable, and your read on
  whether this is refireable vs needs human decision.

The orchestrator's contract: "the runner returned" → "the lifecycle
is complete." If you return earlier than that, you've broken the
contract and the orchestrator will trust the wrong state.

## Related

- [`dispatch-wait-react`](../dispatch-wait-react/SKILL.md) -- the
  background + notification pattern for the CI-watch half of your
  lifecycle.
- [`draft-pr-first`](../draft-pr-first/SKILL.md) -- the full
  lifecycle your invocation must hold open through.
- [`runner-issue-authority`](../runner-issue-authority/SKILL.md) --
  the first step of that lifecycle (fetch the issue body).
