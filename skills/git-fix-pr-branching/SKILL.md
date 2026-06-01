---
name: git-fix-pr-branching
description: When a PR is open and needs a fix, push to the same branch or wait for merge before branching. Never branch off the open PR's branch directly -- squash-merge will leave you with a phantom conflict.
---

# Git fix-PR branching

If an open PR needs a fix, do **one** of:

1. **Push to the same branch.** The fix commit extends the PR.
   Reviewer sees the whole context.
2. **Wait for the open PR to merge, then branch fresh from `main`.**
   Cleanest when the project squash-merges (closer matches the
   eventual history).

Do **NOT** branch off the open PR's feature branch to make a separate
fix PR.

## Why

If the open PR squash-merges, the squash commit and the original
branch's commits have the same content but different SHAs. Any
branch that carries those original commits will conflict against
main even though the content matches. The fix branch then needs to
be rebased onto main to drop the duplicate commits, which is more
work than either option above.

Concrete: caught on roba 2026-05-27. After PR #24 (`test/live-perms`)
merged with squash, the standalone fix PR #25 conflicted with main
because its branch still carried PR #24's original (pre-squash)
commit. Rebasing the fix branch onto main resolved it by dropping
the duplicate.

## How to apply

Before branching to fix a freshly opened PR, check the PR's state:

```bash
gh pr view <PR>  # or gh pr list --state open
```

- **PR open / not merged:** check out its branch, push the fix to
  it.
- **PR merged:** `git checkout main && git pull --ff-only && git
  checkout -b fix/<short-description>`. Never branch from the now-
  closed feature branch.

## Related

- [`git-branch-pr-workflow`](../git-branch-pr-workflow/SKILL.md) --
  the general "branch off main + PR" discipline.
- [`git-delete-merged-branches`](../git-delete-merged-branches/SKILL.md)
  -- once a PR is merged, the local branch is dead weight.
