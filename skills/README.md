# roba skills (Layer 1)

One curated set of knowledge skills for agents that *use* roba from
inside Claude Code. This library is optional -- roba the binary
stands on its own; these skills codify operational guidance that
`roba --help` doesn't cover (prompt-template idioms, PR-lifecycle
steps, observability for when runs go sideways, git workflow
defaults) for the agent-driven multi-task orchestration shape we
found useful. They extend whatever agent loads them.

This is **Layer 1** of the three-layer skill library tracked in
[#47](https://github.com/joshrotenberg/roba/issues/47):

- **Layer 1 (here) -- operational skills.** Markdown knowledge files
  that *extend the parent's context*. Read these once per session;
  apply when relevant.
- **Layer 2 -- runner / orchestrator subagents.** Shipped: see
  [`../agents/`](../agents/). Procedural workers that *consume* these
  skills and run the dispatch lifecycle.
- **Layer 3 -- domain skills.** Bring-your-own. Subagent-format
  markdown (rody's `.claude/agents/` pattern) works straight as
  `roba -f` prompt input; the frontmatter's `tools:` field maps to
  roba's `--allow-tool` flags.

## Available skills

| Skill | When to use |
|---|---|
| [`dispatch-via-bash`](dispatch-via-bash/SKILL.md) | The orchestrator's most important discipline -- dispatch via `Bash` → roba CLI, NOT via the `Task` tool. Honest trade-off table vs Task tool |
| [`orchestration-patterns`](orchestration-patterns/SKILL.md) | The three orchestration patterns -- in-project (P1), workspace (P2), hierarchical (P3) -- and when to use each |
| [`orchestrator-parallelization`](orchestrator-parallelization/SKILL.md) | When to fan out dispatches vs sequentially. Default sequential; parallel when different-file, independent-semantics, predictable-pattern |
| [`dispatch-wait-react`](dispatch-wait-react/SKILL.md) | Coordinating with background tasks (roba runs, CI watches) -- background + harness notification, not poll-and-sleep. Orchestrator-focused; runner discipline cross-referenced |
| [`sandbox-preflight`](sandbox-preflight/SKILL.md) | Start of a runner / bare dispatch -- verify needed tools are in the sandbox allowlist; fail loud (not a "run this yourself" artifact) on a block, auto-heal known-safe dev tools, ask before adding anything else |
| [`release-audit-anchoring`](release-audit-anchoring/SKILL.md) | Release-readiness / release-audit tasks -- anchor analysis on `origin/main` not the working tip, surface branch divergence in the first paragraph, cross-check published versions externally |
| [`runner-issue-authority`](runner-issue-authority/SKILL.md) | The runner's authoritative source for what to do is `gh issue view <N>`, NOT the orchestrator's paraphrase |
| [`runner-synchronous-lifecycle`](runner-synchronous-lifecycle/SKILL.md) | The runner fires roba synchronously and only returns after the full lifecycle is complete (PR pushed, CI running, ready for review) |
| [`draft-pr-first`](draft-pr-first/SKILL.md) | Starting any work that will become a PR -- open the draft PR with the plan as the body BEFORE the work |
| [`roba-orchestration-prompt`](roba-orchestration-prompt/SKILL.md) | Firing roba on the user's behalf and writing the prompt |
| [`roba-spiral-diagnosis`](roba-spiral-diagnosis/SKILL.md) | A roba run hangs, produces no output, or seems stuck |
| [`git-branch-pr-workflow`](git-branch-pr-workflow/SKILL.md) | Any non-trivial work in this repo |
| [`git-fix-pr-branching`](git-fix-pr-branching/SKILL.md) | A PR is open and needs a fix |
| [`git-delete-merged-branches`](git-delete-merged-branches/SKILL.md) | After a PR merges, cleaning up the local branch |
| [`heredoc-backticks`](heredoc-backticks/SKILL.md) | Piping markdown into `gh issue create` / `gh pr create` |

## Installation

These skills are bundled into the roba binary at build time. Install
them into your Claude config so any Claude Code session
auto-discovers them:

```bash
roba skill install            # copy -> ~/.claude/skills/
roba skill install --to .claude/skills   # or a project-local path
roba skill list               # what's bundled, with descriptions
roba skill show draft-pr-first  # print one skill's SKILL.md body
```

`install` flags: `--to PATH`, `--dry-run`, `--force` (overwrite),
`--skip` (leave existing). No network fetch -- the content ships in
the binary.

You can also read the files directly from this directory.

## Format

Each skill is a directory containing a `SKILL.md` with YAML
frontmatter:

```
---
name: <kebab-case-name>
description: <one-line: what it provides + when to invoke>
---

# <Title>

<body>
```

The format is structurally compatible with Claude Code's
`.claude/skills/` convention, so dropping these into that path
"just works."

## Roba does not depend on these

The wrapper stays thin. Skills live alongside, not inside. You can
use roba effectively without loading any of them; they're offered,
not required.

## Composes with aliases

roba *aliases* (`[alias.NAME]` in `roba.toml`) are `git`-style prompt
shortcuts that can carry default flags and pin a subagent. A starter
alias config is a natural companion to `roba skill install` -- the
install command is the right delivery vehicle for opinionated starter
aliases (github verbs, agent bindings) rather than baking them into
the binary. See [`docs/aliases.md`](../docs/aliases.md).
