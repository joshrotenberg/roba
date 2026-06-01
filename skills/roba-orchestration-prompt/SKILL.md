---
name: roba-orchestration-prompt
description: Writing the prompt and managing the PR lifecycle when firing roba on a user's behalf. Apply whenever the user asks for work via roba ("fire roba on X", "have roba handle this") instead of doing the work directly.
---

# Roba orchestration prompt

When the user asks for work via roba, you (the orchestrator) write a
tight prompt, fire roba with it, then manage the PR lifecycle around
the run. Do NOT just pass the user's words through.

This is a stacked-reliability contract:

- **Bottom:** explicit, well-formed prompt
- **Middle:** roba's predictable shell-call surface (typed exit codes,
  JSON ABI, fail-fast on interactive flags, no hidden state)
- **Top:** orchestrator that writes the prompt + manages the PR
  lifecycle around the run

The orchestrator's value-add is the prompt-writing layer plus the
gh-CLI wrapping. Roba's value is staying focused on "do the work";
it shouldn't grow PR-lifecycle verbs.

## Prompt template

```
## Setup

cwd: <absolute path>

Steps 1-3 MUST run sequentially, one tool call at a time. Do NOT
parallel-batch them with each other or with any exploration step.

1. git checkout main
2. git pull --ff-only origin main
3. git checkout -b <type>/<short-description>

Verify with a single Bash call:
   git branch --show-current
Output must be `<branch-name>`. Do NOT re-run steps 1-3.

## Context

<2-4 sentences on what the task is, why it matters,
 and any constraints that aren't obvious from the issue.>

## Task

<Mechanical specifics: file paths, function names, surrounding
 patterns to mirror. If the change touches a known-tricky area,
 spell out the seam.>

## Tool-call discipline

(See companion skill `roba-spiral-diagnosis` -- include this
verbatim or by reference. Without it, parallel-batch cancellation
cascades have a real chance of derailing the run.)

## Steps

1. ...
2. ...
N. cargo fmt --all -- --check
N+1. cargo clippy --all-targets --all-features -- -D warnings
N+2. cargo test --lib --all-features
N+3. cargo test --test cli --all-features
N+4. If all green: git add, commit
       <type>: <short description> (closes #<issue>)
N+5. Print git log --oneline -1, git diff HEAD^ --stat, and the
     branch name.

## Constraints

- Do NOT push.
- Do NOT amend existing commits.
- Do NOT touch main after step 1.
- Do NOT run gh pr create.
- Do NOT modify the live tests (tests/live.rs).
- <task-specific do-nots>
```

## PR-lifecycle pattern (sync-watch-then-merge)

Don't rely on `gh pr merge --auto`. Its behavior depends on repo
settings (`allow_auto_merge`) and can silently no-op or fire
unexpectedly. The sync pattern is portable across repo configs and
leaves a hook for reacting to CI failures.

The loop:

1. Push branch (the one roba created and committed to).
2. `gh pr create --draft` with a body summarizing the change and
   referencing the underlying issue. Include the roba session id for
   traceability where useful.
3. `gh pr ready <PR>` to take it out of draft.
4. `gh pr checks <PR> --watch --interval 15` in **background**
   (`run_in_background=true`). Capture the exit code via the
   notification.
5. On notification:
   - **Exit 0** (all checks passed): `gh pr merge <PR> --squash
     --delete-branch`. Surface the merge commit hash.
   - **Exit non-zero** (one or more checks failed): read the watch
     output for the failing job name, surface failures to the user.
     Optionally fire roba again with the failure context as input
     ("fix the CI failures in PR #X; checkout the branch first").

All orchestrator-side -- pure gh-CLI + roba wrapping. No roba changes
required.

## `--auto` quirk worth remembering

`gh pr merge --auto` silently exits 0 even when `allow_auto_merge:
false` is set on the repo. The PR may or may not actually queue for
auto-merge. Don't rely on it; use the sync pattern above.

## Related

- [`roba-spiral-diagnosis`](../roba-spiral-diagnosis/SKILL.md) --
  what to do when the run hangs.
- [`git-branch-pr-workflow`](../git-branch-pr-workflow/SKILL.md) --
  the "branch off main + PR" discipline the prompt template assumes.
- [`heredoc-backticks`](../heredoc-backticks/SKILL.md) -- how to
  format the PR body in a `gh pr create --body "$(cat <<'EOF' ...
  EOF)"` call without breaking the markdown.
