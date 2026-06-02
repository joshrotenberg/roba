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

For dispatches that will use build tools, `gh`, `git`, or other Bash
commands, include the pre-flight check pattern from
[`../sandbox-preflight/SKILL.md`](../sandbox-preflight/SKILL.md) near
the top of the steps list -- a blocked tool should fail loud, not
silently degrade into a "run this yourself" artifact.

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
N+4. If the work produced anything worth capturing in project
     context (a new decision, a dogfood-worthy outcome, a brainstorm-
     worthy design idea), update CLAUDE.md with the appropriate
     entry. See "Read first, update last" below.
N+5. If all green: git add, commit
       <type>: <short description> (closes #<issue>)
N+6. Print git log --oneline -1, git diff HEAD^ --stat, and the
     branch name.

## Read first, update last

The full read-CLAUDE.md-first / update-CLAUDE.md-last discipline:

- **Read first.** Claude Code auto-loads CLAUDE.md when cwd matches
  the project; this happens transparently as the spawned roba boots.
  No explicit step needed.
- **Update last.** Before the final commit, ask: did this run
  produce something that belongs in CLAUDE.md? Three categories
  worth capturing:
  - **Decisions log entry** -- a settled choice (e.g. "we
    decided X because Y; tracked in #N / PR #M"). One terse line
    under the right date.
  - **Dogfood log entry** -- this run itself, if it was a roba-
    dispatched task. Add a row to the dogfood table: date / target
    / model / clock / spiral y-n / lessons or PR. New lessons
    bubble up to the "Key lessons so far" list.
  - **Brainstorm-sketches addition** -- a design idea that surfaced
    mid-work and is worth capturing for later.
- **Don't update for nothing.** A small refactor that just executes
  the plan doesn't need a CLAUDE.md update. The bar is "would
  future-me want to find this when grepping the durable design
  home?"
- CLAUDE.md is untracked and out of scope for the PR diff, but
  edits to it persist locally and inform the next run.

## Constraints

- Do NOT push.
- Do NOT amend existing commits.
- Do NOT touch main after step 1.
- Do NOT run gh pr create.
- Do NOT modify the live tests (tests/live.rs).
- <task-specific do-nots>
```

## PR-lifecycle pattern (draft-PR-first, sync-watch-then-merge)

The lifecycle is **draft-PR-first** ([see the dedicated
skill](../draft-pr-first/SKILL.md)) -- open the PR before the work
so the plan is visible and the work is observable from minute zero.
Then a sync watch + manual merge on green.

Don't rely on `gh pr merge --auto`. Its behavior depends on repo
settings (`allow_auto_merge`) and can silently no-op or fire
unexpectedly. The sync pattern is portable across repo configs and
leaves a hook for reacting to CI failures.

The full loop:

```bash
# 1. Branch + empty initial commit (so a PR can exist)
git checkout main && git pull --ff-only origin main
git checkout -b <type>/<short-description>
git commit --allow-empty -m "chore: start work on #<N>"

# 2. Push and open the draft PR with the plan as the body
git push -u origin <branch>
gh pr create --draft \
    --title "<conventional commit subject> (closes #<N>)" \
    --body "$(cat /tmp/roba-task-<N>.md)"
# (or compose a separate human-facing plan body if the prompt
# isn't shaped right for human reading)

# 3. Fire roba against the same checkout
roba --fresh --full-auto -C <repo-path> -f /tmp/roba-task-<N>.md

# 4. When roba returns: push the commits it made
git push        # auto-extends the open draft PR

# 5. Mark PR ready
gh pr ready <pr-number>

# 6. CI watch + merge on green
sleep 15        # let GitHub register the checks (dodges the race)
gh pr checks <pr-number> --watch --interval 15

# (in background -- the notification fires when CI completes)
# On exit 0: gh pr merge <pr-number> --squash --delete-branch
# On exit non-zero: read the watch output for failing job names,
#   surface failures, optionally fire roba again with failure
#   context ("fix the CI failures in PR #X; checkout the branch
#   first")
```

The empty initial commit gets squashed away on merge. The plan
lives in the PR body permanently and is observable from anywhere
via `gh pr view <N>`. See [`draft-pr-first`](../draft-pr-first/SKILL.md)
for the full rationale.

All orchestrator-side -- pure gh-CLI + roba wrapping. No roba changes
required.

## `--auto` quirk worth remembering

`gh pr merge --auto` silently exits 0 even when `allow_auto_merge:
false` is set on the repo. The PR may or may not actually queue for
auto-merge. Don't rely on it; use the sync pattern above.

## Related

- [`draft-pr-first`](../draft-pr-first/SKILL.md) -- the "open the
  PR before the work" pattern this skill's PR-lifecycle assumes.
- [`roba-spiral-diagnosis`](../roba-spiral-diagnosis/SKILL.md) --
  what to do when the run hangs.
- [`git-branch-pr-workflow`](../git-branch-pr-workflow/SKILL.md) --
  the "branch off main + PR" discipline the prompt template assumes.
- [`heredoc-backticks`](../heredoc-backticks/SKILL.md) -- how to
  format the PR body in a `gh pr create --body "$(cat <<'EOF' ...
  EOF)"` call without breaking the markdown.
