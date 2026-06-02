---
name: release-audit-anchoring
description: For release-readiness or release-audit tasks, anchor analysis on origin/main (or the project's default branch), not the working branch tip. Surface branch divergence in the first paragraph of the report. Cross-check published versions against external sources, not just in-tree files.
---

# Release audit anchoring

A release-readiness audit is only useful if it targets the branch
that will actually ship. The dangerous failure mode is auditing the
**working branch tip** -- which may be stale, mid-release-prep, or a
side branch that predates a version bump -- and reporting its state
as if it were the release state. The result is a confident audit full
of false blocking findings, which wastes time and undermines trust in
every future audit.

## When to apply

Apply this skill when the task framing involves any of: "release
readiness," "release audit," "is this ready to ship," "next release,"
"version chaos," or similar release-shaped analysis.

When the task says "audit X for release," the meaningful reference
point is the **default branch** (typically `main`), not whatever
branch happens to be checked out. The working tree is a candidate or
a side branch; the thing that ships is `origin/main` (or the project's
release branch).

## Three disciplines

### 1. Anchor on origin/main

Before any analysis, establish the baseline and compute divergence:

```bash
git fetch origin main
git rev-parse origin/main
git rev-parse HEAD
git log --oneline origin/main..HEAD | head -10   # commits ahead
git log --oneline HEAD..origin/main | head -10   # commits behind
```

If you're not on `main`, you now know exactly how the working branch
diverges. Then run the actual analysis **against `origin/main`**, not
the working tree:

```bash
git show origin/main:Cargo.toml | grep version
git show origin/main:CHANGELOG.md | head -30
```

The working tree may be in a stale or in-progress state; `origin/main`
is the ground truth for "what ships."

If the project's default branch is not `main` (e.g. `master`,
`develop`, `release`), substitute it everywhere above. Confirm with
`git remote show origin | grep 'HEAD branch'` if unsure.

### 2. Surface branch divergence in the first paragraph

Open the audit report with the divergence, before any findings:

```
Analyzing branch: <branch> (N commits ahead of origin/main, M behind).

The audit below targets origin/main. The working branch state has
the following divergence: <summary>.
```

This makes it immediately obvious whether the audit is hitting the
right baseline. If the divergence is meaningful (the branch is stale,
contains release-prep commits, or predates a version bump), the user
can redirect before reading a single finding -- instead of discovering
at the end that the whole audit targeted the wrong tree.

### 3. Cross-check published versions externally

Never determine the published version from in-tree files alone.
`Cargo.toml`, `CHANGELOG.md`, and `README.md` are frequently ahead of
or behind the registry depending on release-plz (or equivalent) state.

Cross-check against an external source of truth:

- Registry latest version:
  - Rust: `cargo search <crate>`
  - Python: `pip index versions <pkg>`
  - Node: `npm view <pkg> versions`
- GitHub releases / tags:
  `gh release list --limit 5` or
  `git tag -l --sort=-v:refname | head -5`

Reconcile the in-tree version, the registry version, and the latest
tag/release. A mismatch is usually release-plz state (in-tree bumped
ahead of a not-yet-published release), not "version chaos" -- name it
as such rather than flagging it as a blocking inconsistency.

## Worked example

The #114 failure mode -- a bare dispatch audited a stale docs branch's
tip and reported in-tree version skew as blocking findings:

```
Bad:

> "Version chaos: Cargo.toml -> 0.8.7, README -> 0.9, CHANGELOG has
> 'release v0.9.0' nested under [0.8.7]. The task framing assumes
> v0.9.0 shipped but nothing in the repo is actually at 0.9.0."
```

In reality v0.9.0 had shipped; `origin/main` was clean at 0.9.0. The
analyzed branch was a stale docs branch that predated the release-plz
bump. With this skill applied:

```
Good:

> "Analyzing branch: docs/refresh-readme-and-examples (3 commits ahead
> of origin/main, 0 behind). Note: origin/main is at 0.9.0 (Cargo.toml,
> README, CHANGELOG aligned). The working branch is a stale docs branch
> that predates the v0.9.0 release-plz bump; its Cargo.toml at 0.8.7
> reflects pre-release state.
>
> Audit against origin/main: PASS -- ..."
```

The user immediately sees the audit is hitting the right baseline and
that the apparent "version chaos" was just a stale side branch.

## Related

- [`sandbox-preflight`](../sandbox-preflight/SKILL.md) -- the other
  "the audit ran but produced nothing trustworthy" failure mode
  (blocked build tools turning a release audit into BLOCKED entries).
- [`roba-orchestration-prompt`](../roba-orchestration-prompt/SKILL.md)
  -- pull this skill into the prompt's discipline section for any
  release-shaped task.
