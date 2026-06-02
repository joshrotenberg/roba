---
name: orchestration-patterns
description: The three orchestration patterns -- in-project (P1), workspace (P2), hierarchical (P3) -- and when to use each. Patterns nest; a single session may use more than one. Choose the pattern that matches the work, don't conflate P1 + P2.
---

# Three orchestration patterns

You can operate in one of three patterns. Patterns nest -- a single
session may use more than one.

## Pattern 1: In-project orchestration (default for focused work)

You're *in* the project: cwd matches the project root. You dispatch
roba within the same repo. Single context, lowest overhead. The
common case when the user wants focused work on one project.

```
[interactive session, cwd = /path/to/project]
   │
   ▼
roba -C . -f task.md
   │
   ▼
spawned claude (same project, same CLAUDE.md auto-loads)
```

## Pattern 2: Workspace orchestrator (multi-project)

Your cwd sits above N projects (or is unrelated to any of them).
You dispatch `roba -C <project>` per project. Each spawned claude
auto-loads its project's CLAUDE.md. You coordinate cross-repo state
(in-flight PRs, dispatch order, blockers, parallel vs sequential).

```
[session, cwd = above N projects]
   │
   ├──▶ roba -C /path/to/A -f task.md  (spawned claude loads A's CLAUDE.md)
   ├──▶ roba -C /path/to/B -f task.md  (spawned claude loads B's CLAUDE.md)
   └──▶ roba -C /path/to/C -f task.md  (different stack, different domain)
```

`gh pr list --draft` per repo gives the cross-repo state of work in
flight. The draft-PR-first lifecycle ensures any session can pick up
where another left off via the durable GitHub state.

**Known gap:** Claude Code's CLAUDE.md discovery walks up *within* a
project; it doesn't pick up cross-project context. Workspace-level
context (the orchestrator's home) is in your session-level context,
not in the spawned workers'. Tracked as roba #97; document the
limitation when a worker needs context it doesn't have.

## Pattern 3: Hierarchical orchestration (delegation of orchestration)

You dispatch roba to a project with `--agent=<project-orchestrator>`,
so the spawned claude IS that project's own orchestrator. It
internally dispatches sub-roba runs to do the actual work. You don't
need to know the project's backlog details; the project orchestrator
does.

```
[top orchestrator, cwd = anywhere]
   │
   ▼
roba -C /path/to/A --agent=A-orchestrator -f "work the next 3 issues"
   │
   ▼
spawned claude (= A's project orchestrator)
   │
   ├──▶ roba -C . -f task-1.md
   ├──▶ roba -C . -f task-2.md
   └──▶ roba -C . -f task-3.md
          │
          ▼
        spawned worker (runs the actual code change)
```

Costs and gaps:

- Token cost multiplies per level (~3x a Pattern-1 run).
- Speculative: no real-use signal yet. Overkill for most tasks.
- Right shape for release coordination across a workspace,
  cross-cutting refactors needing per-project policy, workspace-
  level audits where the top orchestrator coordinates *without*
  knowing per-project details.
- Each level should still externalize state through GitHub PRs --
  the top orchestrator should be able to read the project
  orchestrator's progress via `gh pr list --draft` without
  inspecting the spawned claude session.

## Which pattern is this dispatch?

When you receive a directive, identify which pattern fits:

- **One project, focused work** -> Pattern 1
- **Multiple projects, you're sequencing the work yourself** ->
  Pattern 2
- **Multiple projects, each with its own backlog complexity that
  warrants its own orchestrator** -> Pattern 3 (rare; verify with
  the user before assuming)

The patterns compose. A single session can drive Pattern 1 on the
active project while dispatching Pattern 2 calls to adjacent repos
in the background.

## Related

- [`dispatch-via-bash`](../dispatch-via-bash/SKILL.md) -- the
  dispatch mechanism each pattern relies on.
- [`orchestrator-parallelization`](../orchestrator-parallelization/SKILL.md)
  -- when to fan out within a pattern.
