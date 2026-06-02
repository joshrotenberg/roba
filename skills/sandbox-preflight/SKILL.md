---
name: sandbox-preflight
description: At the start of a roba lifecycle (runner or bare dispatch), verify the tools you need are in the sandbox allowlist. Fail LOUD on a blocked tool -- do NOT produce a "run this yourself" artifact (the silent-degradation trap). For a known-safe set of common dev tools, auto-heal by adding to .claude/settings.local.json. Anything else: ask the user.
---

# Sandbox preflight

Before doing any real work, verify that the tools you'll need are
actually runnable in the sandbox. The dangerous failure mode this
prevents is **silent degradation**: a blocked tool (`gh`, `cargo`,
etc.) makes the agent emit an output that *looks* complete -- a
"run this command yourself" markdown artifact, or a report full of
`BLOCKED` lines -- while no real state changed. The orchestrator
gets a "completed" signal and trusts the wrong state.

Two work-machine reports surfaced the same root cause:

- **roba #112:** the runner returned a "run this yourself" markdown
  block when `gh` wasn't in the allowlist. Output looked finished;
  no GitHub state changed; the synchronous-lifecycle "complete"
  contract silently broke.
- **roba #113:** a bare release-audit dispatch ran static checks but
  couldn't run the build gate (`cargo fmt/clippy/test`,
  `maturin build`) because those tools weren't allowlisted. It asked
  "want me to add the allowlist?" as a question and accumulated
  `BLOCKED` entries instead of acting.

Both are sandbox-permission issues. The fix is one discipline:
**check first, fail loud, auto-heal the known-safe set, ask for
anything else.**

## When to apply

- **Runner step 0.** Before fetching the issue, before composing any
  prompt. Referenced from
  [`../../agents/roba-runner/AGENT.md`](../../agents/roba-runner/AGENT.md).
- **Bare roba dispatches** that will use build tools, `gh`, `git`,
  or any tool beyond `Read`/`Glob`/`Grep`. The
  [`roba-orchestration-prompt`](../roba-orchestration-prompt/SKILL.md)
  skill should add a pre-flight section near the top of such a
  prompt's steps.

If the work is genuinely read-only (a Q&A or explainer that only
needs `Read`/`Glob`/`Grep`), no pre-flight is needed.

## Tools to verify

A minimum set the runner always needs:

- `gh` -- GitHub CLI (issue / PR / CI-watch operations)
- `git` -- branch + commit + push

Plus project-detected tools, keyed off files in the repo root:

| Detected file | Tools to verify |
|---|---|
| `Cargo.toml` | `cargo` |
| `pyproject.toml` / `setup.py` | `pip`, `uv`, `maturin` (whichever the project references) |
| `package.json` | `npm` (and `pnpm` / `yarn` if its lockfile is present) |
| `go.mod` | `go` |
| `Makefile` / `build.sh` | `make` / `bash` |

Check each by attempting a no-op version probe -- `<tool> --version`
(or a tool-appropriate equivalent). If the call is **blocked** by the
sandbox (not merely "command not found"), the tool is not in the
allowlist.

## The fail-loud rule

**Never produce a "run this yourself" artifact.** If a tool you need
is blocked AND it is not in the auto-heal allowlist below, ABORT the
lifecycle with a clear message and do NOT proceed:

```
ABORTED at sandbox preflight: <tool> is not in the sandbox allowlist.

To unblock, add the following to .claude/settings.local.json under
"permissions.allow":

  "Bash(<tool>:*)"

Then re-dispatch this task.
```

The orchestrator sees an **abort**, not a fake "complete." That is
exactly what the synchronous-lifecycle contract requires for trust:
a blocked run is a legitimate hand-back, not a silently degraded
success.

## Auto-heal allowlist

For these well-known dev tools, self-heal by adding the appropriate
`Bash(<tool>:*)` entry to `.claude/settings.local.json` instead of
aborting:

- `gh`, `git`
- `cargo` (when `Cargo.toml` is present)
- `npm`, `pnpm`, `yarn` (when `package.json` is present)
- `pip`, `uv`, `maturin` (when `pyproject.toml` / `setup.py` is
  present)
- `go` (when `go.mod` is present)
- `make`, `bash`, `sh` (always -- they're the universal build glue)

Self-healing is a **contained** action: it writes a single allowlist
entry, surfaces what it added in the return summary, and proceeds.
The user sees the addition in the returned report -- nothing happens
silently.

Anything outside this list: **ASK the user before adding.** Tools
like `docker`, `kubectl`, `terraform`, or custom scripts are
security-sensitive enough to require explicit consent. Don't
auto-heal them; surface the need and wait.

## What to surface

- **Pre-flight passes cleanly:** say nothing; proceed.
- **Pre-flight self-heals:** include in the return summary
  `Auto-healed: added Bash(<tool>:*) to .claude/settings.local.json`.
- **Pre-flight aborts:** emit the ABORT message above; do NOT proceed
  with the rest of the lifecycle.

## Related

- [`runner-synchronous-lifecycle`](../runner-synchronous-lifecycle/SKILL.md)
  -- why a blocked run must hand back honestly instead of returning a
  fake "complete."
- [`roba-orchestration-prompt`](../roba-orchestration-prompt/SKILL.md)
  -- where to put the pre-flight section in a bare-dispatch prompt.
- [`roba-spiral-diagnosis`](../roba-spiral-diagnosis/SKILL.md) -- the
  other "the run looked busy but produced nothing real" failure mode.
