---
name: git-branch-pr-workflow
description: All non-trivial work goes through a feature branch + PR. Never commit directly to main, even for solo projects. Apply by default before any edit.
---

# Git branch + PR workflow

Default to `git checkout -b <type>/<description>` **before any
edits**. Never commit directly to main, even on solo projects.
Conventional commit prefixes: `feat`, `fix`, `refactor`, `chore`,
`ci`, `docs`, `test`. Push branch, open PR, let the user (or the
orchestrator loop) merge.

## When to apply

- Any code change beyond a trivial typo
- Any docs change that affects committed content
- Any config or workflow change

## Why

- Keeps the commit log on main clean and bisectable
- CI runs against the PR before main moves
- Squash-merge gives main a clean linear history
- Even solo, the PR body is a useful place to record the "why"

## How to apply

1. Before any change, `git checkout -b <type>/<short-description>`.
2. Make commits with conventional-commit messages
   (`<type>: <description>`; `!` marks breaking).
3. Push: `git push -u origin <branch>`.
4. Open the PR: `gh pr create [--draft]` with a tight body that
   references the underlying issue (use a `Closes #N` keyword in the
   PR body so the merge closes the linked issue).
5. Wait for CI; merge with `gh pr merge --squash --delete-branch`.

## Commit-message conventions

- `feat: ...` -- new user-visible behavior
- `fix: ...` -- bug fix
- `refactor: ...` -- internal restructuring, no behavior change
- `chore: ...` -- repo maintenance (dependencies, config, etc.)
- `ci: ...` -- CI/release-process changes
- `docs: ...` -- documentation only
- `test: ...` -- tests only

Append `!` to mark a breaking change: `refactor!: cut --head and
--tail (closes #42)`.

Do not include "Generated with Claude Code" or "Co-Authored-By"
trailers.

## Related

- [`git-fix-pr-branching`](../git-fix-pr-branching/SKILL.md) -- how
  to handle fixes when a PR is open vs merged.
- [`git-delete-merged-branches`](../git-delete-merged-branches/SKILL.md)
  -- the cleanup default after a merge.
- [`heredoc-backticks`](../heredoc-backticks/SKILL.md) -- formatting
  the PR body without breaking the markdown.
