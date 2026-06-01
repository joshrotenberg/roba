---
name: roba-spiral-diagnosis
description: When a roba run hangs, produces no output, or seems stuck. Read the spawned claude session jsonl directly -- it's the source of truth. Echo-flush spirals usually trace to one failed parallel tool call cascading cancellation errors to siblings.
---

# Roba spiral diagnosis

Roba's stdout/stderr capture is unreliable as a debug signal. The
spawned claude session writes a normal jsonl under
`~/.claude/projects/<the-same-project-dir>/<spawned-session-id>.jsonl`,
and **that file is the ground truth** for what the agent did.

## When to apply

- A `roba` run has been quiet for an unusual length of time relative
  to the task size
- The bash capture for the run is 0 bytes or empty
- You want to confirm what the spawned agent is actually doing
  *while* it runs

## How to find the spawned session

The spawned claude session writes to the SAME project directory as
the parent (roba inherits cwd via `-C`, claude-code keys session
storage by cwd):

```
~/.claude/projects/<encoded-project-path>/<uuid>.jsonl
```

Find the latest non-this-session entry:

```bash
ls -lt ~/.claude/projects/<this-project-dir>/*.jsonl | head -3
```

The biggest recent file that *isn't* the current Claude Code session
is the spawned roba's session.

## What to look for

Parse `type == 'assistant'` entries, then `content[].type ==
'tool_use'` for what the agent has been doing:

```python
import json
fn = '~/.claude/projects/<dir>/<spawned-uuid>.jsonl'
for line in open(fn):
    r = json.loads(line)
    if r.get('type') != 'assistant':
        continue
    c = r.get('message', {}).get('content', [])
    if not isinstance(c, list):
        continue
    for b in c:
        if isinstance(b, dict) and b.get('type') == 'tool_use':
            ts = r.get('timestamp', '')[:19]
            print(f"{ts} {b.get('name')}: "
                  f"{json.dumps(b.get('input', {}))[:120]}")
```

## Spiral signatures (kill the run early)

These patterns indicate the agent has lost the thread:

1. **Echo-flush spam.** Many consecutive `echo flush` or `echo fb1 /
   fb2 / fb3...` Bash tool calls. The agent thinks tool output is
   missing and is trying to "flush" something.
2. **Cancellation cascade.** Repeated `<tool_use_error>Cancelled:
   parallel tool call ...</tool_use_error>` tool_result entries.
   One sibling parallel call errored; the framework cancelled all
   the others.
3. **Parallel-batch timestamp collisions.** Many tool calls in a
   single assistant turn all sharing the same wall-clock timestamp.
   The agent is batching aggressively, which is exactly what
   triggers (2).

Kill with `kill <roba-pid> <wrapper-bash-pid>` (ps aux | grep roba
to find them).

## Root cause (almost always)

Echo-flush spirals are NOT a roba bug. Roba is a thin wrapper:
compose prompt, `claude_wrapper::QueryCommand`, `execute_json`,
print the result. No tool-result handling.

The cause is inside the spawned claude:

1. Agent batches a parallel turn that re-runs a setup command
   (commonly `git checkout -b <branch>` it already created in an
   earlier turn).
2. The duplicate fails (e.g. exit 128 `branch already exists`).
3. Claude's tool framework cancels every other call in that parallel
   batch, returning `<tool_use_error>Cancelled: parallel tool call
   Bash(...) errored</tool_use_error>` to all of them.
4. Agent sees a wall of cancellations and misreads it as "tool
   output is missing," goes into flush-spiral mode.

## Prevention (in the prompt)

The orchestration prompt should include this `## Tool-call
discipline` section verbatim:

```
- Setup steps (git checkout, pull, branch) must run sequentially,
  NOT in a parallel batch with exploration.
- Before re-running any setup command, verify state first
  (`git branch --show-current`, `git status`).
- If tool calls return `<tool_use_error>Cancelled: parallel tool
  call Bash(...) errored</tool_use_error>` errors, do NOT retry
  blindly. Do NOT issue "flush" echo commands. Read the actual
  failing call, decide if it matters, fix or continue. Almost
  always: the failure is a duplicate setup command, and the cure
  is to STOP issuing the duplicate, not to flush.
```

Also: pass `--fresh` to the roba invocation so the spawned claude
starts clean rather than inheriting any prior session state.

## `--worktree` is effectively `--fresh`

`-w` / `--worktree` creates a new git worktree (different cwd).
Claude sessions are keyed by cwd, so a worktreed run won't pick up
any prior session -- effectively fresh by cwd isolation. But it
adds worktree-management complexity for the orchestrator; prefer
`--fresh` unless the task genuinely benefits from a sandbox
worktree.

## Related

- [`roba-orchestration-prompt`](../roba-orchestration-prompt/SKILL.md) --
  the full prompt template that incorporates the prevention rules
  above.
