# roba use cases

A cookbook of patterns `roba` enables. The entries run in loose order
from "single-task pipeline" toward "multi-task agent orchestration."
Each one is self-contained -- pick the one that matches the work in
front of you.

## Contents

- [Multi-repo orchestration](#multi-repo-orchestration) -- drive 3-5
  repos at once with a coordinating agent

## Multi-repo orchestration

**Driving 3-5 repos at once with a coordinating agent. Two layers,
kept separate.**

In this pattern an orchestrating agent fans work out across several
repos, and each unit of work runs through a single roba invocation.
The same flags that make roba usable on a TTY -- typed exit codes,
clean stdout, the `--json` envelope -- are what let an agent dispatch
work and branch on the result.

The pattern lives or dies on keeping two layers apart.

### The two layers

- **Orchestration bus = headless `roba` calls.** A delegate-and-collect
  loop: invoke `roba` per repo (in that repo's directory, or via `-C`),
  branch on the typed exit codes (`0/1/2/3/4`), collect the `--json`
  envelope. This layer is the actual coordination. It runs fine with
  **no tmux at all** -- CI, ssh, cron.
- **Human cockpit = tmux (optional, not load-bearing).** One window per
  repo so a developer can cycle in, watch, and take over. The
  orchestrator does **not** `send-keys` into these windows; coordination
  never routes through them. Each window just *observes* state the
  orchestrator produces (for example, `tail -f` on a per-repo log) or
  fires a one-shot `roba -c=ID "..."` to re-enter a specific thread by
hand. Swap
  tmux for zellij, a dashboard, or nothing -- the bus is unchanged.

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

Do **not** make tmux `send-keys` between live TUIs *be* the bus. It
has no completion signal, no structured result, and blocks on
permission prompts -- exactly the problems the headless `roba` path
solves. Keystroke injection between terminals gives the orchestrator
no way to know a task finished, no parseable result, and no failure
class to branch on. The cockpit is for observation; the bus is the
typed `roba` invocations plus their JSON.

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

The agent reads each repo's JSON envelope, branches on the exit code,
and keeps going even when individual repos fail. No tmux is involved.
The `--json` shape is the versioned envelope: on success the answer is
at `.result.result` under `version: 1`; on failure the envelope carries
an `.error` object with `.error.message` and a typed `.error.exit_code`
(see the [JSON output contract](../README.md#versioned-json-output)).

### Why this is roba-specific

The features that make this pattern viable are roba's, not plain
`claude -p`:

- **Typed exit codes** -> the orchestrator branches on failure class
  (auth / budget / timeout) instead of regexing prose.
- **Clean stdout/stderr split + `--json` envelope** -> a structured
  result with no scraping; stdout is the answer, everything else is on
  stderr.
- **Profiles** -> per-repo policy lives *with the repo*. Each repo's own
  `roba.toml` defines what is allowed, so the orchestrator stays thin.
- **`roba cost --by-project`** -> token usage aggregated per repo across
  history.
- **Fail-fast on interactive flags without a TTY** -> a headless run
  never silently hangs waiting on a prompt that can't arrive.
- **Per-repo self-contained context** -> each repo's `CLAUDE.md`,
  `skills/`, `.claude/agents/`, and `roba.toml` auto-load when roba runs
  with `-C <path>`. The orchestrator just dispatches; the spawned worker
  lands in the right project context with no extra wiring.

The agent-ABI surface these rely on is tracked across issues #33-#37
and documented in the repo `README.md` and `CLAUDE.md`.

### The cockpit is throwaway

Pick whatever observation tool you like. tmux is cheap, lightweight,
and lets a human cycle into any repo to watch or take over. zellij
works the same way. A simple dashboard tailing the per-repo logs works.
Or skip the cockpit entirely when running headless -- CI, cron, a
remote box over ssh. The bus does not care which (if any) you use,
because nothing about coordination flows through it.

### The source-sink-context model

The bus pattern works without per-project setup because a project
that already has an issue tracker, a PR workflow, a `CLAUDE.md`, and
source carries everything a single roba dispatch needs:

| role | location | what it provides |
|---|---|---|
| **Source (input)** | GitHub issue | what to do; the prompt is derived from it |
| **Sink (output)** | GitHub PR | where the plan lives (body), how progress is observable (commits), and how it is reviewed and merged |
| **Context (background)** | per-repo `CLAUDE.md` + source code | how this project works |

That is the complete data flow for one task. The orchestrator wires
source and context into a tight prompt, fires roba, and manages the
PR lifecycle. The orchestrator iterates over projects, and each one is
a complete, self-contained unit.

---

More use cases will land here as they emerge. The patterns above
generalize -- if you have a use case that fits roba's shape (one-shot
dispatch, structured output, typed failures), open an issue describing
it.
