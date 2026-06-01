# roba agents (Layer 2)

Spawnable subagent definitions for orchestrating work with roba. Each
agent is a markdown file with YAML frontmatter (`name`, `description`,
`tools`, `model`) plus a system-prompt body, structurally compatible
with Claude Code's `.claude/agents/` convention.

This is Layer 2 of the three-layer skill library tracked in
[#47](https://github.com/joshrotenberg/roba/issues/47):

- **Layer 1 -- knowledge skills** at [`skills/`](../skills/). Extend the
  parent agent's context with how-to.
- **Layer 2 (here) -- runner / orchestrator subagents.** Procedural
  workers that *consume* the Layer 1 skills and do the lifecycle.
- **Layer 3 -- domain skills.** Bring-your-own (reviewer, test-writer,
  etc.). The orchestrator + runner are agnostic to which domain
  agents you have; they just dispatch.

## Available agents

| Agent | Scope | Invoke with |
|---|---|---|
| [`roba-runner`](roba-runner/AGENT.md) | One task: implement a single issue end-to-end | `@roba-runner implement #N` |
| [`roba-orchestrator`](roba-orchestrator/AGENT.md) | Many tasks: plan / sequence / parallelize work across one or more repos | `@roba-orchestrator work the backlog in foo, bar` |

## Relationship

```
user directive
      │
      ▼
  orchestrator         ── queue-level: plans, sequences, parallelizes,
      │                   surveys state, surfaces blockers
      ▼ delegates per task
   runner              ── task-level: reads the issue, composes the
      │                   prompt, runs the draft-PR-first lifecycle,
      ▼ dispatches      reports back
    roba                ── substrate: deterministic dispatch
      │
      ▼
  spawned claude       ── executes against the project's CLAUDE.md +
                          skills + source
```

The orchestrator manages WHAT to do; the runner does HOW; roba is the
deterministic substrate; the spawned claude does the actual code work.

## Installation

These agents are bundled into the roba binary at build time. Install
them into your Claude config so any Claude Code session can spawn
them:

```bash
roba agent install            # copy -> ~/.claude/agents/
roba agent install --to .claude/agents   # or a project-local path
roba agent list               # what's bundled, with descriptions
roba agent show roba-runner   # print one agent's AGENT.md body
```

`install` flags: `--to PATH`, `--dry-run`, `--force` (overwrite),
`--skip` (leave existing). The `roba skill install` command does the
same for the Layer 1 skills. No network fetch -- the content ships in
the binary.

## Format

Each agent is a directory containing `AGENT.md` with YAML frontmatter:

```
---
name: <kebab-case-name>
description: <one-line: what it does + how to invoke>
tools: <comma-separated tool list>
model: <model id or "inherit">
---

# <Title>

<system-prompt body>
```

The format is structurally compatible with Claude Code's
`.claude/agents/` convention, so dropping these into that path
"just works."

## Roba does not depend on these

The wrapper stays thin. Agents live alongside, not inside. You can
use roba effectively without loading any of them; they're offered, not
required. The runner / orchestrator simply codify patterns that worked
during dogfooding (see CLAUDE.md's dogfood log).

## Composes with aliases

A roba *alias* (`[alias.NAME]` in `roba.toml`) can pin one of these
agents via its `agent` field, turning the agent into a discoverable
verb -- e.g. `roba implement 42` binding to `roba-runner`. A starter
alias config that binds these agents is a natural companion to
`roba agent install`; see the Aliases section of
[`docs/profiles.md`](../docs/profiles.md#aliases).
