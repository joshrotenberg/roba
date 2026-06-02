# roba use cases

What `roba`'s shape enables. Most of the value is in straightforward
one-shot invocations -- `roba "..."` on a TTY, `roba --json
"..."` in a script. This page focuses on the use case where roba's
feature composition matters most.

## Multi-repo orchestration

**Driving N repos at once with a coordinating agent.** The features
that make roba viable here -- typed exit codes, clean stdout, the
`--json` envelope, `-C <path>` -- are exactly the ones that make it
usable on a TTY. The same surface serves both audiences.

The pattern lives or dies on keeping two layers apart.

### The two layers

- **Orchestration bus = headless `roba` calls.** Delegate-and-collect:
  invoke `roba` per repo (with `-C <path>`), branch on the typed exit
  codes (`0/1/2/3/4`), parse the `--json` envelope. This is the actual
  coordination. It runs fine with no terminal at all -- CI, ssh, cron.
- **Human cockpit = tmux (optional, not load-bearing).** One window
  per repo so a developer can cycle in and watch. The orchestrator
  does **not** `send-keys` into these windows. Each window just
  *observes* state the bus produces (e.g. `tail -f` on a per-repo
  log). Swap tmux for zellij, a dashboard, or nothing -- the bus
  is unchanged.

```text
  ORCHESTRATION BUS (load-bearing)
  the agent delegates and collects; typed exit codes + --json

    agent ──roba --json -C repo-a──▶ repo-a
          ──roba --json -C repo-b──▶ repo-b
          ──roba --json -C repo-c──▶ repo-c

  HUMAN COCKPIT (optional, observe-only)

    ┌──────────┐ ┌──────────┐ ┌──────────┐
    │ tmux: a  │ │ tmux: b  │ │ tmux: c  │
    │ tail -f  │ │ tail -f  │ │ tail -f  │
    └────┬─────┘ └────┬─────┘ └────┬─────┘
         │ observes   │ observes   │ observes
         ▼            ▼            ▼
       repo-a       repo-b       repo-c

  The windows watch state the bus produces. They do not drive it.
```

### Anti-pattern: tmux send-keys as the bus

Do **not** make tmux `send-keys` between live TUIs *be* the bus. No
completion signal, no structured result, blocks on permission
prompts -- exactly the problems the headless `roba` path solves.
Keystroke injection between terminals gives the orchestrator no way
to know a task finished, no parseable result, and no failure class
to branch on. The cockpit is for observation; the bus is the typed
`roba` invocations plus their JSON.

### A minimal delegate-and-collect example

```bash
# delegate.sh
repos=( ~/Code/a ~/Code/b ~/Code/c )
for repo in "${repos[@]}"; do
  result=$(roba --json -C "$repo" "review the recent changes")
  exit_code=$?
  case $exit_code in
    0) echo "$repo: ok"
       echo "$result" | jq -r '.result.result' > "$repo/.review.md" ;;
    2) echo "$repo: auth -- run claude login" ;;
    3) echo "$repo: budget exceeded" ;;
    4) echo "$repo: timeout" ;;
    *) echo "$repo: failed ($exit_code) -- $(echo "$result" | jq -r '.error.message')" ;;
  esac
done
```

The agent reads each repo's JSON envelope, branches on the exit
code, and keeps going even when individual repos fail. No tmux
involved. The `--json` envelope shape: on success the answer is at
`.result.result` under `version: 1`; on failure the envelope carries
an `.error` object with `.error.message` and a typed
`.error.exit_code` (see [scripting.md](scripting.md#versioned-json-envelope)).

### What makes this roba-shaped

The features that make this pattern viable are roba's, not plain
`claude -p`:

- **Typed exit codes** -> the orchestrator branches on failure class
  (auth / budget / timeout) instead of regexing prose.
- **Clean stdout/stderr split + `--json` envelope** -> a structured
  result with no scraping; stdout is the answer, everything else is
  on stderr.
- **`-C <path>`** -> each spawned worker lands in the right project
  context (project `CLAUDE.md` auto-loads when cwd matches).
- **Profiles** -> per-repo policy lives *with the repo*. Each repo's
  own `roba.toml` defines what's allowed.
- **Fail-fast on interactive flags without a TTY** -> a headless
  run never silently hangs waiting on a prompt that can't arrive.
- **`--trace PATH`** -> JSONL of the spawned session's events for
  observability mid-flight. Combined with the dispatch-start session
  id line, the orchestrator always has a handle to what's happening.
- **`roba cost --by-project`** -> token usage aggregated per repo
  across history.

### Where the agent layer lives

This page is about roba's substrate role -- dispatch, structured
output, typed failures. The actual orchestrator + runner agent
definitions (the things that decide *what* to dispatch and follow
the PR lifecycle through merge) are out of scope for the binary
and live separately. [joshrotenberg/agent-tools](https://github.com/joshrotenberg/agent-tools)
is one curated set: dispatcher / runner subagents plus operational
skills covering draft-PR-first lifecycle, sandbox preflight,
spiral diagnosis, parallelization, and the source-sink-context
model. Drop them into `~/.claude/{skills,agents}/` and any Claude
Code session can spawn them.

A bare roba CLI works without any of that -- the substrate is
useful on its own. The agent layer adds discipline on top.
