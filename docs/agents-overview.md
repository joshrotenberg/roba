# Agents and orchestration

roba bundles an optional library of operational skills and two
orchestrator subagents that codify one curated convention for driving
multi-task, multi-repo work from inside Claude Code. The binary works
fine without it -- this library is the "batteries included" add-on for
teams that want a ready-made orchestration pattern.

## What it is

The library has two layers:

- **Layer 1: Skills** -- operational knowledge files loaded by Claude
  Code when it enters a directory. Each skill in `skills/` covers one
  topic: how to open a draft PR, how to diagnose a roba spiral, how to
  anchor a release audit. They're markdown files with YAML frontmatter.
- **Layer 2: Agents** -- Claude Code subagents defined in `agents/`.
  Two are bundled: `roba-orchestrator` (plans and dispatches work across
  issues and repos) and `roba-runner` (executes one issue at a time,
  owns the full PR lifecycle).

This is one curated convention, not the only way to use roba. Bring
your own skills and agents if the patterns don't match your workflow.

## Install

```bash
roba skill install    # copy bundled skills -> ~/.claude/skills/
roba agent install   # copy bundled agents -> ~/.claude/agents/
```

Both commands accept `--to PATH` (custom destination), `--dry-run`
(preview without writing), `--force` (overwrite existing), and `--skip`
(leave existing in place, install the rest). Without a flag, an
existing file prompts before overwriting on a TTY; it's left alone
when there's no TTY.

After install, any Claude Code session (not just roba) auto-discovers
the skills and agents via the standard `~/.claude/` lookup.

The install-and-go path:

```bash
cargo install roba
roba skill install && roba agent install
claude --agent=roba-orchestrator
```

## How it composes with Claude Code

`--agent NAME` pins a Claude Code subagent for a roba dispatch. The
named agent must exist in `.claude/agents/NAME.md` in the cwd (or be
auto-discovered via claude's standard walk-up). The bundled agents
declare their required skills in their frontmatter, so they
self-configure when claude loads them.

```bash
roba --agent roba-runner -f /tmp/task.md   # dispatch via runner
```

Each repo can carry its own agents and skills in `.claude/agents/` and
`.claude/skills/` (or in the project's own `skills/` and `agents/`
directories, installed globally). The spawned claude inherits the
project's context automatically via cwd-keyed discovery; the
orchestrator only needs `-C <path>` to land in the right project.

## The orchestrator -> runner pattern

The bundled convention is a two-level hierarchy:

1. **`roba-orchestrator`** lives in an interactive Claude Code session.
   It reads the issue tracker, decides what to work on, composes a
   prompt file, opens a draft PR, and fires `roba --fresh --full-auto
   -f /tmp/task.md` for each task.
2. **`roba-runner`** executes a single task end-to-end: reads the
   issue, explores the codebase, implements the change, pushes,
   watches CI, and merges.

State lives outside the conversation: issues are the work queue, draft
PRs are the in-flight state, `CLAUDE.md` and `skills/` carry project
context. The conversation is disposable; the work survives.

Three orchestration patterns are documented in the
`orchestration-patterns` skill (see Skills below):

- **In-project** -- orchestrator and worker in the same repo.
- **Workspace** -- one session dispatches `roba -C <project>` to N
  repos.
- **Hierarchical** -- a top orchestrator dispatches per-project
  orchestrators, which dispatch workers.

## URL addressability

Every bundled skill and agent has a canonical URL pair -- a rendered
page on the docs site and a raw markdown URL for `WebFetch`:

```bash
roba skill show draft-pr-first --url    # prints rendered + raw URLs
roba agent show roba-runner --url

roba skill list --urls     # table of all skills with URL columns
roba agent list --urls
```

The raw URL is suitable for an agent to `WebFetch` the latest source
without requiring the binary to be up to date.

## See also

- **Skills** -- the full library is listed on the [Skills](../skills/README.md)
  page. Each skill is self-contained -- you can read it directly, install
  it into a project, or point at it with `--url` for live fetch.
- **Agents** -- the runner and orchestrator are documented on the
  [Agents](../agents/README.md) page with their full lifecycle and
  failure-handling notes.
- **Use cases** -- worked examples of the orchestration pattern in
  practice: [`use-cases.md`](use-cases.md).
