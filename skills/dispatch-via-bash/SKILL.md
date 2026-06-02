---
name: dispatch-via-bash
description: Dispatch code-changing work via `Bash` → roba CLI, NOT via the Claude Code `Task` (Agent) tool. The process boundary is what gives you context isolation, history, cost tracking, --trace observability, the agent-ABI envelope, and worktree isolation. The Task tool with `subagent_type: "roba-runner"` silently defeats all of that.
---

# Dispatch via Bash, NOT via the Task tool

**The most important orchestrator discipline IF you're using the
bundled `roba-orchestrator` agent.** Getting it wrong silently
defeats the substrate the orchestrator is built on.

## Where this fits

This skill applies when you've loaded the
[`roba-orchestrator`](../../agents/roba-orchestrator/AGENT.md)
agent and want the in-Claude-Code multi-task orchestration pattern.

It does NOT apply if:

- You're using roba directly from a shell or script (no orchestrator
  in the loop -- just `roba "..."`).
- You're using Claude Code's built-in background agents or agent
  teams for multi-session work (those have their own mechanics and
  their own trade-offs; roba's substrate isn't in the picture).
- You're using `Task(subagent_type: "Explore" / "Plan" / ...)` for
  read-only delegation that doesn't need code changes or
  observability outside Claude Code's session model.

roba and Claude Code's native agent mechanisms (Task, background
agents, agent teams) are different shapes -- not strictly better or
worse than each other. The trade-off table below assumes you've
already chosen "use the roba-orchestrator pattern"; what it actually
spells out is "given that choice, here's how dispatch flows."

## The substrate boundary is the value

When you dispatch a task, the work runs in a **separate process**
(roba spawns claude in its own session). That process boundary is
what gives you:

- **Context isolation.** Each task's 30k+ tokens of grep/read/edit
  churn lives in the spawned claude's context, not yours. Your
  context stays slim. You can run 20 tasks across an evening and
  still respond cleanly; the spawned sessions are throwaway.
- **`roba history` visibility.** Every dispatch is a real session
  in `~/.claude/projects/<dir>/`, queryable, resumable.
- **`roba cost` accounting.** Token usage rolls up. You can answer
  "what did this batch cost?"
- **Worktree isolation** (`-w` flag) for parallel runs.
- **`roba.toml` config** is applied per-dispatch.
- **`--trace PATH`** observability handle for in-flight runs.
- **Versioned JSON envelope, typed exit codes, structured error
  envelope** -- the agent-ABI for programmatic reaction.

If you dispatch via the wrong tool, **NONE** of this works.

## The two correct dispatch shapes

### Shape A: Fire roba with `--agent roba-runner` (default for managed tasks)

```bash
roba --fresh --full-auto -C <repo-path> --agent roba-runner -f /tmp/roba-task-<N>.md
```

The `--agent roba-runner` flag (shipped as roba #84) tells roba to
spawn claude acting as the `roba-runner` subagent. The runner's
lifecycle discipline (gh issue view, draft-PR-first, synchronous
lifecycle, etc.) applies inside roba's spawned session.

Wait for completion per
[`dispatch-wait-react`](../dispatch-wait-react/SKILL.md).

### Shape B: Fire roba directly without `--agent` (ad-hoc tasks)

For tasks that are small / ad-hoc / don't need the runner's full
lifecycle (a quick docs sweep, a config tweak):

```bash
roba --fresh --full-auto -C <repo-path> -f /tmp/roba-task-<N>.md
```

The spawned claude follows the prompt directly. No runner agent
discipline -- the prompt must be tight on its own.

## THE ANTI-PATTERN

```
Task(subagent_type: "roba-runner", description: "implement #N", prompt: "...")
```

**Do NOT do this.** It looks correct -- the runner subagent gets
the task -- but it spawns the runner IN YOUR CONTEXT (Claude Code's
Agent-tool subagent mechanism, not roba's process boundary). The
runner does the work, your session accumulates all of it, and
**you've bypassed every value listed above**:

- No `roba history` entry
- No `roba cost` accounting
- No worktree isolation possible
- No `roba.toml` applied
- No `--trace` JSONL
- No structured envelope
- Your context bloats with the task's grep/read/edit churn

**If you find yourself reaching for the Task tool with
`subagent_type: "roba-runner"`, stop.** That subagent is meant to
be spawned via `roba --agent roba-runner`, not via the in-process
Agent tool.

## When the Task tool IS valid

The Task tool is in your tools list for legitimate uses:

- `Task(subagent_type: "Explore", ...)` -- delegate a read-only
  search/survey without spawning roba (no code change involved).
- `Task(subagent_type: "Plan", ...)` -- delegate a planning pass.
- Other non-roba subagents the user has set up.

Heuristic: **if the work would produce code changes / commits /
PRs, it goes through `Bash` → `roba`. If it's read-only research /
exploration / planning / Q&A, the Task tool with a non-roba
subagent is fine.**

## Roba vs Task tool: honest trade-offs

The Task tool isn't *wrong* for everything. It's wrong for
*code-changing work as an orchestrator*. Here's the honest tally:

| dimension | `Bash` → roba | `Task` (Agent tool) |
|---|---|---|
| **Context isolation** | Work runs in a separate claude session; your context stays slim | Subagent runs in your process; its full grep/read/edit churn lives in YOUR context |
| **Cross-task scale** | Run 20 tasks across an evening, your session is still responsive | After 5-10 tasks, you're hitting context limits |
| **`roba history` / `roba cost`** | Every dispatch is a real session, queryable + cost-rolled-up | Subagent invocation is invisible to roba; no history, no cost record |
| **Worktree isolation** | `-w=NAME` per dispatch; safe parallel runs | Not possible -- shares your working tree |
| **Per-repo `roba.toml` / CLAUDE.md** | Auto-applied when spawned claude lands in the project | Subagent inherits YOUR cwd / config |
| **`--trace PATH` observability** | Stable handle to tail / read events as JSONL | Not available |
| **Versioned JSON envelope + typed exit codes** | Yes -- agent-ABI for programmatic reaction | No -- subagent returns free-text |
| **Spawn cost** | ~1-2 seconds startup + token cost of warming the spawned session | Near-zero; subagent shares your token budget for context warmup |
| **Mid-run iteration** | You wait for completion (peek trace to monitor); to tweak prompt you wait + refire | You can react mid-conversation; faster iteration loop |
| **Killing a runaway** | `kill PID` -- your session survives | Killing the subagent's call is harder; bad runs can poison your context |
| **Setup required** | `roba` binary must be installed; for `--agent NAME` the named subagent must be in `~/.claude/agents/` | Just Claude Code; no separate install |
| **Right tool for** | Code-changing work, multi-task orchestration, anything you want isolated and observable | Read-only research (`Explore`), planning passes (`Plan`), quick in-context Q&A delegation |

**Bottom line:** the Agent tool is genuinely cheaper for one-off
in-context delegations. Roba's cost (spawn time, less mid-run
iteration) buys you the substrate that makes multi-task
orchestration viable -- the same way using `git` costs more per
change than just editing files, but you don't try to manage a
project without it.

Use the Agent tool when you'd be fine with the cost / state living
in your own session (one-shot research, planning). Use roba when
you need the work to be isolated, queryable, resumable, observable,
and not crowding your context.

## Default tool for dispatch

**Bash.** Always. Every roba dispatch starts as a Bash call. The
Task tool comes out only for the non-roba, read-only cases above.

## Related

- [`dispatch-wait-react`](../dispatch-wait-react/SKILL.md) -- the
  background + harness notification pattern for waiting on a
  dispatched roba run.
- [`roba-orchestration-prompt`](../roba-orchestration-prompt/SKILL.md)
  -- the prompt template for the file you pass to `-f`.
- [`draft-pr-first`](../draft-pr-first/SKILL.md) -- the lifecycle
  that wraps a roba dispatch (branch + draft PR + dispatch + push +
  ready + merge).
