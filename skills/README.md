# roba skills (Layer 1)

Knowledge skills for agents that *use* roba. These extend whatever
agent loads them (Claude Code in-session, or a roba-orchestrating
parent agent) with operational guidance that `roba --help` doesn't
cover: prompt-template idioms, PR-lifecycle steps, observability for
when runs go sideways, git workflow defaults.

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
the binary. See the Aliases section of
[`docs/profiles.md`](../docs/profiles.md#aliases).
