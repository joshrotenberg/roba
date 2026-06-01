---
name: git-delete-merged-branches
description: After a PR merges and main is synced, delete the local feature branch with `git branch -d`. GitHub auto-deletes the remote side. Don't ask -- this is the default.
---

# Delete merged branches

After a PR is merged, run:

```bash
git branch -d <feature-branch>
```

without asking. GitHub is typically configured to auto-delete the
remote side, so the local deletion is the only loose end after a
merge.

## When to apply

After every PR merge, once `main` is synced:

```bash
gh pr merge <PR> --squash --delete-branch  # also deletes remote
git checkout main
git pull --ff-only origin main
git branch -d <feature-branch>
```

## Use -D sparingly

Use `git branch -D` (capital D, force) only if `-d` refuses. Refusal
means git can't verify the branch is merged into the current branch,
which deserves a pause -- verify the merge state before forcing.

## Related

- [`git-branch-pr-workflow`](../git-branch-pr-workflow/SKILL.md) --
  the upstream "branch + PR" discipline.
- [`git-fix-pr-branching`](../git-fix-pr-branching/SKILL.md) --
  what to do if you need to make a fix and the PR's state is
  ambiguous.
