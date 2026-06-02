---
name: dispatch-wait-react
description: How to coordinate with background tasks (roba runs, CI watches, sub-agent dispatches) without polling or sleep-looping. Fire in background, wait for the harness notification, peek at in-flight output deliberately, surface stalled runs after a reasonable clock budget.
---

# Dispatch, wait, react

When you fire a long-running command -- a roba dispatch, a `gh pr
checks --watch`, a sub-agent run -- the right coordination pattern
is **background + notification, not poll-and-sleep.** This skill
codifies the mechanism that turns "I fired something and now I'm
idle" into "I fired something, here's exactly how I track progress
and act on completion."

## Who this is for

**Primarily the orchestrator** (the interactive session with the
user). The orchestrator wants to stay responsive to the user while
roba runs, CI watches, etc. The "background + notification" pattern
serves that.

**The runner subagent is different.** A runner subagent invocation
must complete the FULL lifecycle (fire roba, push, mark ready,
watch CI, merge or surface) before returning to the orchestrator,
because the runner's return signals "task done." If the runner
backgrounds roba and returns early, the lifecycle is orphaned --
exactly the failure mode in #104. The runner should fire roba
**synchronously** (no `run_in_background`) and only return once the
lifecycle is done. See
[`../../agents/roba-runner/AGENT.md`](../../agents/roba-runner/AGENT.md)
for the runner-specific discipline.

## When to apply (orchestrator)

- Any roba dispatch you fire directly (`roba --full-auto -f ...`)
- Any CI watch (`gh pr checks <PR> --watch`)
- Any sub-agent invocation (including `@roba-runner`)
- Any other Bash command that takes >5 seconds and produces a result
  you'll act on

## The shape

### 1. Always fire with `Bash(run_in_background=true)`

You get back a task ID + an output-file path. Do NOT fire long-
running commands without `run_in_background`; that blocks your
turn waiting and burns budget on the same context. The default
should be: if it might take >5s, background it.

```
Bash(command="roba --fresh --full-auto -C /path -f task.md", run_in_background=true)
=> Background job b7jv3db8o; output at /private/tmp/.../b7jv3db8o.output
```

### 2. Don't sleep-poll

Don't write a `while not_done; sleep 10; check; end` loop. The
harness will send a `<task-notification>` automatically when the
background command exits, carrying the task ID, exit status, and
output-file path. Your job is to **wait for that notification**,
not to invent a polling loop.

If you find yourself reaching for `sleep` to wait on a background
job, stop -- you're rebuilding what the harness already provides
for free.

### 3. Peek at progress deliberately, not on every turn

Mid-run, you CAN read the output file via the Read tool to see
what's been produced so far. That's free -- no spawned cost, just
file IO. Good triggers to peek:

- **The user asks "what's going on?"** -- read the output and
  surface a short status.
- **You suspect a spiral** -- a small task that should take 2-3
  minutes has been running for 10+. Peek the output AND read the
  spawned claude session jsonl per
  [`../roba-spiral-diagnosis/SKILL.md`](../roba-spiral-diagnosis/SKILL.md)
  to identify echo-flush spam, cancellation cascades, or other
  diagnostic signatures.
- **You're chaining and want to surface partial info** -- e.g.
  the user is on remote control and you want to give them a brief
  "PR #N is in flight, agent is reading source files now."

Peek **deliberately**, not on every turn. Constantly reading the
output file converts coordination into polling -- wasted cycles
and noise in the transcript.

### 4. React to the notification

When the harness sends a `<task-notification>` for your job's
task ID, that's your cue to act. The notification carries:

- The task ID (matches what you got at launch)
- Status (completed / failed / killed)
- Exit code (in the failed case, often)
- The path to the output file

Act based on the exit code:

- **0 / completed cleanly** -- proceed with the next step in your
  lifecycle (push commits, mark PR ready, merge, etc).
- **Non-zero / failed** -- read the output file, decide refire vs
  hand-back per
  [`../roba-spiral-diagnosis/SKILL.md`](../roba-spiral-diagnosis/SKILL.md)
  (for roba runs) or the specific failure modes section of your
  agent's body (for other tools).

### 5. Have a clock budget

For each background task, have a rough sense of "too long":

| task | rough budget | reaction at 2x budget |
|---|---|---|
| Small roba dispatch (doc edit, small flag add) | 2-4 min | peek output + jsonl for spiral signatures |
| Medium roba dispatch (multi-file refactor, new module) | 5-10 min | same |
| Large roba dispatch (substantial new feature) | 10-15 min | same |
| `gh pr checks --watch` (modern roba CI) | 2-5 min | check PR state; maybe merged externally |
| Sub-agent invocation | varies | depends on the sub-agent's scope |

If the harness hasn't notified by 2x the rough budget, **peek
first** before assuming a hang. Then surface to the user with the
status + your read on what's happening.

Don't silently wait past 3x budget without surfacing -- that's a
real hang or a notification miss, and the user should know.

### 6. Composition

This skill sits underneath several lifecycle skills + agents:

- [`draft-pr-first`](../draft-pr-first/SKILL.md) -- fire roba ->
  wait -> push -> ready -> watch CI -> wait -> merge. Each "wait"
  is the pattern in this skill.
- [`roba-orchestration-prompt`](../roba-orchestration-prompt/SKILL.md) --
  the PR-lifecycle pattern that includes `gh pr checks --watch` in
  background. Same wait shape.
- The orchestrator agent's parallelization heuristics
  ([`../../agents/roba-orchestrator/AGENT.md`](../../agents/roba-orchestrator/AGENT.md))
  -- fire N background tasks; the notification fan-in handles
  which finishes first. **Wait for ANY notification**, then
  handle that one job, then go back to waiting.
- [`roba-spiral-diagnosis`](../roba-spiral-diagnosis/SKILL.md) --
  when a wait is "too long" or the output file looks weird, peek
  the spawned jsonl. That skill describes the diagnostic side;
  this one describes the coordination side.

## Anti-patterns

- **Foreground long-running commands.** Blocks your turn, wastes
  budget, makes you unresponsive. Always background.
- **Sleep-polling.** Reinventing what the harness gives you for
  free. The notification is automatic; trust it.
- **Constant output-file polling.** Reading the output file every
  turn is polling-by-another-name. Peek deliberately.
- **Silent indefinite waiting.** If a notification is taking too
  long, surface to the user with what you know. Don't sit on a
  stalled job in silence.
- **Firing serial when the work is parallel-safe.** Per the
  orchestrator's parallelization heuristics, when work is
  independent and different-file, fan out with background tasks
  -- the notification fan-in handles the join.

## Related

- [`draft-pr-first`](../draft-pr-first/SKILL.md) -- the PR
  lifecycle this skill enables.
- [`roba-orchestration-prompt`](../roba-orchestration-prompt/SKILL.md)
  -- the orchestrator's full prompt + PR-lifecycle pattern.
- [`roba-spiral-diagnosis`](../roba-spiral-diagnosis/SKILL.md) --
  what to do when "wait" is "too long."
