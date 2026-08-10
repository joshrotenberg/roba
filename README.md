# roba

[![Crates.io](https://img.shields.io/crates/v/roba.svg)](https://crates.io/crates/roba)
[![Documentation](https://docs.rs/roba/badge.svg)](https://docs.rs/roba)
[![CI](https://github.com/joshrotenberg/roba/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/roba/actions/workflows/ci.yml)
[![Downloads](https://img.shields.io/crates/d/roba.svg)](https://crates.io/crates/roba)
[![License](https://img.shields.io/crates/l/roba.svg)](#license)

A library-first, bounded agent run. One Roba process owns one intention,
may steer its provider through multiple turns, and exits when the work is
complete. Claude Code and Codex are provider adapters; the CLI, REPL, and
run-scoped MCP server are thin clients of the same process-local run API.

Roba is not a daemon or a persistent session pool. A suspended run does no
provider work until its first prompt arrives. While it is alive, a caller can
observe, steer, cancel, and await it through Rust, a REPL, or MCP.

The established single-prompt Claude CLI remains available as a compatibility
surface while it migrates onto the new library model.

## Bounded runs

The new `roba run` path is the first end-to-end slice of the library design:

```bash
# One blocking run. Stdout is the final answer.
roba run --provider codex "inspect this repository and propose the next task"

# Create a suspended run and provide its first prompt from the REPL.
roba run --provider claude --repl

# Expose one suspended run as a run-scoped MCP server over stdio.
mcp-repl -- roba run --provider codex --mcp

# Permit an attached MCP client to create up to four process-local workers.
mcp-repl -- roba run --provider codex --mcp \
  --max-workers 4 --max-worker-depth 2

# Let the root orchestrator create bounded workers itself.
roba run --provider codex --max-workers 4 --max-worker-depth 2 \
  "Investigate the failure, delegate independent checks, and return one answer."

# Resolve defaults, provider policy, and a named agent from one small file.
roba run --config examples/run-config/roba.toml --agent builder \
  "Implement the next coherent change and verify it."
```

Worker creation is disabled unless both bounds are explicit. Workers inherit
the parent's permissions, tools, provider limits, and worker policy, and begin
with a fresh provider session. Terminal worker snapshots remain observable for
the run lifetime. When a parent ends, Roba cancels its live descendants before
the parent becomes terminal, so a bounded run cannot leave provider work
behind. Provider limits apply to each child independently; `max_workers` and
`max_worker_depth` bound the tree, but they are not an aggregate spend limit.
When workers are enabled, each Claude or Codex provider turn receives a private
`roba_workers` MCP server with only `spawn_worker` and `workers`. The server is
bound to that exact run, authenticated on an ephemeral loopback listener, and
removed when the turn ends. Its bearer is transient and is never stored in the
run specification or snapshots.
Roba disables Claude's native Agent tool and Codex's native multi-agent feature
on this provider-neutral run path. Otherwise provider-owned subagents could do
work outside the worker tree's count, depth, cancellation, and event contract.
The adapters also direct the model to use only Roba's private worker tools and
to report a refused spawn instead of launching shell or native substitutes.

The run-scoped MCP server also exposes a read-only `events` tool. Events are
sequenced across the root and its worker tree and retained in a bounded
in-memory journal. Pass the returned `next_sequence` as the next `after`
cursor; an optional `wait_ms` performs bounded long polling. A client that
falls behind receives `truncated: true` rather than an incomplete history
presented as complete, and long polling returns that gap immediately. Cursors
ahead of the current journal are refused. Claude and Codex output and usage are
emitted into this journal as each provider's JSONL stream arrives.

Configuration resolves in one hierarchy:

```text
Roba defaults -> selected provider defaults -> named agent -> run overrides
```

`roba run --config PATH` loads the public TOML shape directly. It has only
`[defaults]`, `[providers.NAME]`, and `[agents.NAME]`; CLI flags are the final
run overrides. Unknown fields fail before provider work. This explicit run
config is separate from the legacy one-shot CLI's discovered `roba.toml`
profiles while that compatibility surface remains available. See
[`examples/run-config/roba.toml`](examples/run-config/roba.toml).

The public implementation is split by responsibility:

- `roba-core`: provider-neutral specifications, resolution, provider registry,
  lifecycle, outcomes, and events
- `roba-mcp`: run-scoped observation and control over MCP
- `roba-repl`: line-oriented control over the same `RunHandle`
- `roba`: the CLI and the compatibility surface for the original Claude runner

See [the run-library design](docs/design/run-library-pivot.md) for the current
contract, completed work, and remaining migration plan.

## Legacy one-shot CLI

The original command remains a composable single-prompt runner on top of
`claude -p`, with pipe-clean output, re-enterable sessions, and a stable
scripting ABI.

- **Humans:** prompt from files / stdin / git context, rendered markdown
  on a TTY, flag bundles as profiles, history, cost.
- **Agents** ([Claude Code](https://github.com/anthropics/claude-code)):
  stdout is the answer, stderr is metadata, a versioned `--json`
  envelope, typed exit codes, `--trace` to watch a run.

Built on [`claude-wrapper`](https://crates.io/crates/claude-wrapper) and
[`codex-wrapper`](https://crates.io/crates/codex-wrapper).

```console
$ roba "summarize the rust ownership model in 3 bullets"
   Rust's ownership model rests on three rules:

     • Each value has a single owner.
     • When the owner goes out of scope, the value is dropped.
     • Borrows are either many immutable or one mutable.

tokens 1.2k/450 . $0.0042 . 2.0s . session abc12345
```

## Install

| Source | How |
|---|---|
| crates.io | `cargo install roba` |
| Homebrew | `brew install joshrotenberg/brew/roba` |
| Prebuilt binary | [latest release](https://github.com/joshrotenberg/roba/releases/latest) -- macOS (arm64 / x86_64), Linux (arm64 / x86_64), Windows; shell and PowerShell installers included |

The legacy command shells out to `claude`. `roba run` shells out to the
selected `claude` or `codex` binary. Install and authenticate the provider you
select and keep it on `PATH`.

**The full flag, env-var, and config reference lives in the binary:
`roba --help`.** This README is the overview.

## Legacy CLI vs. `claude -p` directly

`claude -p` is the one-shot primitive: prompt in, response to stdout,
exit. roba keeps that model and adds:

| Adds | How |
|---|---|
| **Composable input** | `-f` file, piped stdin (the prompt, or context when a prompt is given), `-e` editor, `--prepend` / `--append`, `--attach` globs, `--git-diff` / `--git-log` / `--git-status`, `--var` template vars |
| **Pipe-clean output** | stdout is the answer; all metadata (footer, spinner, tool lines, warnings) goes to stderr -- piping to `jq` sees a clean stream |
| **TTY rendering** | markdown, spinner, color while it runs; `--plain` / `NO_COLOR` turns it off |
| **Session re-entry** | `-c` continue, `-c ID` resume, `--fork`, `--pick` chooser, `--session-id` mints a caller-chosen id, `--session NAME` resumes a named handle (`[session]` in roba.toml); `roba history` / `roba last` browse past runs |
| **Read-only inspection** | `roba show <ID>` prints a stored run's result (`--metrics`, `--wait`); `roba worktree list`; `roba history --worktree NAME` finds a runner's session |
| **A stable scripting ABI** | typed exit codes, versioned `--json` envelope, clean stream split -- see [For agents & scripts](#for-agents--scripts) |

These details describe the compatibility command. The bounded `roba run`
surface is provider-neutral and supports Claude and Codex.

## Quick examples

```bash
# Just ask
roba "what's the difference between Arc and Rc?"

# Compose: preamble + question + appendix
roba --prepend system.md "review this design" --append context.md

# Pull in files by glob
roba --attach 'src/**/*.rs' "is the error handling consistent?"

# Read-only review against the working-tree diff
roba --readonly --git-diff "is this safe to merge?"

# Continue the most recent session here (pass the prompt with -p, since
# -c takes an optional session id)
roba -c -p "now show me how to test the unsafe variant"

# scripted multi-turn: mint an id once, reuse it on later turns
# (bare `claude -p --continue` no-ops in print mode; this is the fix)
uuid=$(uuidgen)
roba --session-id "$uuid" "start a refactor plan"
roba -c="$uuid" "now do step 1"

# Pipe-friendly: answer only, stdin in
roba "what's 2+2" -q            # prints "4"
echo "summarize this" | roba    # stdin works, no flag needed
cat err.log | roba "what's wrong here?"   # piped data becomes context
```

> [!NOTE]
> `-c` (continue) and `-w` (worktree) take an *optional* value, so a
> space-separated word right after them is read as that value:
> `roba -c "follow up"` treats `follow up` as the session id. Pass the
> prompt explicitly with `-p` to disambiguate: `roba -c -p "follow up"`.

## Safe by default

roba starts read-only: claude may use `Read`, `Glob`, and `Grep` and
nothing else. You opt into more, explicitly:

```bash
roba "explain this"                      # read-only (default)
roba --writable "rename foo to bar"      # add Edit + Write
roba --allow-tool "Bash(git:*)" "..."    # add one specific pattern
roba --deny-tool WebFetch "..."          # block one (deny wins)
roba --add-dir ../shared "..."           # widen file-tool scope to another dir
roba --full-auto "..."                   # bypass every check (sandbox only)
roba --show-permissions --profile review # preview the resolved set, then exit
```

`--add-dir` (repeatable) passes through to claude's `--add-dir`: claude's
file tools are cwd-scoped by default; each `--add-dir` adds one directory.

The read-only start does not regress. `--permission-mode` additionally
sets claude's own approval mode (`plan`, `acceptEdits`, ...), orthogonal
to the allow-list. Precedence across all layers: **CLI flag > `ROBA_*`
env > profile > top-level config keys > built-in default**, and deny
always wins over allow.

To give claude extra tools from an MCP server for one run, point it at
a server config file:

```bash
roba --mcp-config mcp.json "..."                  # add those servers' tools
roba --mcp-config mcp.json --strict-mcp-config "..." # use ONLY those servers
```

`--mcp-config` (repeatable) passes through to claude's `--mcp-config`:
roba forwards the path, claude reads it. Not a roba MCP server -- it
wires per-run MCP servers into the `claude -p` call.

## Configuration: profiles & aliases

A `roba.toml` lets you stop retyping flags and define your own verbs.
Files are discovered by walking up from the cwd (plus
`~/.config/roba.toml`); closer-to-cwd wins per key.

- **Profiles** are named bundles of flag defaults: `--profile review`
  applies `[profile.review]`. A `default` profile auto-applies.
- **Aliases** are new verbs: `roba review 42` expands an
  `[alias.review]` prompt template (`${1}` / `${pr}` / `$(...)` shell
  substitution) plus default flags and dispatches like a normal call.
  Your domain knowledge lives in your aliases, not the binary.
- **Personas** are profiles that carry a role: set `agent = "NAME"` in
  a `[profile.NAME]` and the run adopts that native claude agent
  (`.claude/agents/NAME.md`) plus the profile's envelope. Inspect them
  with `roba persona list` / `roba persona show NAME`.
- **Draft one with claude:** `roba alias draft "..."` /
  `roba profile draft "..."` generate a validated `[alias.NAME]` /
  `[profile.NAME]` block from a description on stdout (pipe it to
  `>> roba.toml`); `--write` appends it for you.

The fully-commented [`roba-config.sample.toml`](roba-config.sample.toml)
documents every key with worked examples; `roba profile init` drops it
in your project. Inspect with `roba profile {list,show,active}` and
`roba alias {list,show}`.

For ready-to-copy setups, [`examples/`](examples/) carries vetted bundles
(each parse-tested in CI): [`roba-rust-dispatch.toml`](examples/roba-rust-dispatch.toml)
-- a worker-dispatch config proven on a real backlog run, with `fix` and
`design` aliases; [`roba-reviewer.toml`](examples/roba-reviewer.toml), an
enforced read-only, schema-validated reviewer in a few lines of TOML; and
[`roba-multi-task-worker.toml`](examples/roba-multi-task-worker.toml), a
worker sized to chew through a task list in one run (the one-turn-many-tasks
pattern).

## Sealed runs

By default a run inherits everything ambient: roba's config pool walks
up from the cwd, and claude reads `~/.claude` (agents, skills,
CLAUDE.md, MCP servers). For a reproducible, auditable run you can seal
either side or both:

```bash
roba --setting-sources user "..."   # claude ignores project/local ambient config
roba --hermetic "..."               # seal BOTH: skip roba's pool + ~/.config,
                                    #   and pin claude to a known promptspace
roba --hermetic=roba "..."          # or seal one axis: roba | claude
roba --bundle path/.roba "..."      # a .roba/ bundle supplies the run's config
                                    #   (roba.toml, system-prompt.md, mcp.json)
```

Under `--hermetic`, a `./.roba` bundle (when present) is the sole config
source, MCP is strict to what the bundle provides, and claude's dynamic
system-prompt sections are excluded -- the run's capability surface is
exactly what was declared, which is what makes an unattended `--full-auto`
run auditable.

## Worker lifecycle

A single issue moves through four verbs: `issue` (plan, read-only) ->
`ship` (implement to a draft PR) -> `revise` (address feedback) ->
`review` (post the verdict to the PR). You, or a thin loop, drive the
sequence; the watch-and-merge tail stays in `gh`.

Where to put a gate: the agent cooks freely through reversible work,
including fetching the issue, planning, editing, and opening the draft
PR. Gate only at the irreversible or outward edges (CI is green, the
merge) and at flagged judgment that should not be made unilaterally. The
verbs are the cook zone; the driver holds the gates.

[`examples/roba-worker-lifecycle.toml`](examples/roba-worker-lifecycle.toml)
is the copy-to-use bundle: fill its `<GATE>` / `<CONVENTIONS>` /
`<CI-NOTE>` holes and go. Its comments carry the durable-context
discipline (post the verdict via `gh pr comment --edit-last`, not
`gh pr review`; write complete PR bodies; clarify on the issue rather
than guess -- both which fix, and whether to do it at all when the
issue flags its own value as uncertain) and the driver loop.

## For agents & scripts

The contract: **stdout is the answer, stderr is everything else**, and
`--json` wraps every output in a versioned envelope.

```text
success: { "version": 1, "result": { ... }, "refusal": bool }    (stdout)
failure: { "version": 1, "error": { kind, message, exit_code, chain } }    (stderr)
```

The read-only management commands (`cost`, `history`, `last`, `doctor`,
`worktree list`, `jobs`) emit the same `{ "version": 1, "result": ... }` shape
(minus the ask-only `refusal`); `roba show` reconstructs the full ask
envelope, `refusal` included. One parser handles every `--json` output.
Pin `version` and you've pinned the shape.

Reading the envelope:

| Fact | Detail |
|---|---|
| The answer | `.result.result` -- not `.result` (that's the whole object) |
| Metrics | nest under `.result`: `.result.duration_ms`, `.result.num_turns`, `.result.total_cost_usd` (serde rename of `cost_usd`); the top-level paths return `null` |
| Top level | only `version`, `refusal`, and `result` |
| Refusal | still exits `0` (the call succeeded) -- detect via the `refusal` field, not the exit code |
| Exit codes | `0` ok, `1` generic, `2` auth, `3` budget (wrapper tracker), `4` timeout, `5` max-turns, `6` no usable output (empty answer or `is_error`), `7` max-budget cap; `5` and `7` are recoverable cap hits. The error `kind` maps the same way |
| Validate content, not just `$?` | exit `0` means the call returned, not that the answer is usable -- but exit `6` now catches the empty / `is_error` case so a non-answer never looks like success |
| `see_also` | omitted when empty -- don't assume the key exists |

```bash
out=$(roba --json "..."); echo "$out" | jq -r '.result.result'
```

Worker flags:

| Flag | Does |
|---|---|
| `--json-schema PATH` | schema-validated model output; roba reads the file and inlines it (claude's flag wants inline JSON). With `--json`, the answer is surfaced clean: `.result.structured_output` holds the parsed object and `.result.result` is unfenced -- no `\| jq '.result.result' \| sed ... \| jq` fence-stripping needed |
| `--max-turns N`, `--max-budget-usd USD` | rails for unattended loops; hitting a cap errors the run with a recoverable code (`5` max-turns, `7` max-budget) so the caller can finish the lifecycle |
| `--timeout SECS` | wall-clock deadline; on expiry roba kills the child and exits `4`. The rail for a `claude -p` that HANGS (where the turn/budget caps, which bound work not time, never trip). `0` disables; composes with the caps above |
| `--no-retry` | surface transient failures immediately; the caller owns retry |
| `--trace PATH` | the spawned session's events as JSONL -- watch a run in flight |
| `--fallback-model MODEL` | retry on a second model when the primary is overloaded |
| `--no-session-persistence` | run without writing a resumable session record |
| `--full-auto` | unsupervised editing worker; add `--worktree` for parallel same-repo workers (for orchestrator-owned branches use `git worktree add` + `-C <dir>` instead -- roba's `--worktree` is claude-managed, on a branch you won't PR from) |
| `roba doctor --json` | boundary checks as `{ checks: [{ name, status, message }], overall }`; exits `0` when no check fails, `1` otherwise |

Near the end of a turn the trace carries claude's `post_turn_summary`
system event (`status_category` + `status_detail`) -- a usable done /
what-happened signal, but it is claude's event passed through, not part
of roba's versioned ABI.

### Unattended / CI

The flags above are the parts; this is how they assemble for a run with
nobody watching. None of it is new behavior -- it is the existing ABI
applied as one recipe.

- **Read the envelope, not `$?`.** Run with `--json`, then check
  `.error` is absent (or `.is_error`) and `.result.result` is non-empty
  before acting on the answer. Exit `6` now catches the empty / `is_error`
  case, so the exit code and the content agree -- but validating both is
  the belt-and-suspenders an unattended run wants.
- **Exit `6` never emits a stderr error envelope.** The
  `{ version, error }` stderr envelope appears only for the `Err`-path
  codes (`1`-`5`), which have no stdout. Exit `6` never emits one -- the
  reliable signal is the exit code itself. What is on stdout depends on the
  subcase: for an empty / `is_error` result the success-shaped
  `{ version, result }` envelope is already on stdout (inspect `.result` /
  `.is_error`); for a streaming / `--trace` run that produced no result
  event, stdout is empty. Branch on the exit code; read the stdout envelope
  when present; never scrape stderr for an error on code `6`.
- **`--bare` for reproducible runs.** Skips hooks, LSP, plugin sync,
  CLAUDE.md auto-discovery, auto-memory, and keychain reads, so the run
  depends only on the prompt and flags you passed (auth via
  `ANTHROPIC_API_KEY`). Supply context explicitly with `--attach` /
  `--prepend` / `-f` rather than relying on ambient discovery.
- **`--safe-mode` for untrusted input.** When the prompt carries content
  you do not control (an issue body, a PR diff, a scraped page) into a
  `--full-auto` worker, add `--safe-mode`. It starts claude with every
  customization disabled -- CLAUDE.md, skills, plugins, hooks, MCP servers,
  custom commands and agents (`CLAUDE_CODE_SAFE_MODE=1`) -- shrinking the
  custom-code surface a prompt-injection could reach. It is a *security*
  posture, distinct from `--bare`'s reproducibility, and composes with it.
- **The cap trio.** `--max-turns` and `--max-budget-usd` bound *work*;
  `--timeout SECS` bounds *wall-clock* and is the only one that catches a
  `claude -p` that hangs making no progress. Set all three for a run you
  will not babysit; they compose as independent caps.
- **`--trace PATH` to a timestamped file** (e.g.
  `--trace "runs/$(date +%Y%m%dT%H%M%S).jsonl"`) so every run leaves a
  replayable record of the spawned session's events.
- **Branch on the typed exit code:**

| Code | Meaning | Unattended response |
|---|---|---|
| `0` | ok (refusals included -- check the `refusal` field) | use `.result.result` |
| `1` | generic failure | fail the job |
| `2` | auth | fail loudly; do not retry |
| `3` | budget (wrapper `BudgetTracker`) | fail the job |
| `4` | timeout (`--timeout` expired) | the run hung; retry or escalate |
| `5` | max-turns (recoverable -- work may be committed) | inspect, finish the lifecycle |
| `6` | no usable output (empty answer or `is_error`) | treat as failure, not success |
| `7` | max-budget cap (recoverable -- work may be committed) | inspect, finish the lifecycle |

A minimal guard:

```bash
out=$(roba --bare --json --max-turns 80 --max-budget-usd 10 --timeout 600 \
  --trace "runs/$(date +%Y%m%dT%H%M%S).jsonl" -f task.md)
code=$?
if [ "$code" -ne 0 ]; then echo "roba failed: exit $code" >&2; exit "$code"; fi
answer=$(printf '%s' "$out" | jq -er '.result.result') || { echo "empty answer" >&2; exit 1; }
```

If claude is configured to pause on an approval hook (its `defer`
mechanism), the turn ends with no usable answer and roba returns. That is
claude's gate, not roba's; resume the paused work in a later run by
pinning the same handle (`--session-id` / a named `[session]`), exactly
as for any other re-entry.

### Several tasks in one run

roba runs exactly one `claude -p` turn, and a turn can hold many tasks.
Inside a single invocation the agent loops over tools until it is done,
so a prompt that lists several tasks gets worked sequentially in that one
turn -- there is no chaining flag and no special mode. Size the rails to
the batch: `--max-turns` and `--max-budget-usd` are per-run caps, so
multi-task work needs more runway than a single edit (a change that
cascades across files can easily want `--max-turns 80`, not the default).
A `[profile.worker]` in your `roba.toml` is the usual home for those caps.

What roba does *not* do is span turns. There is no
fire-a-task / end-the-turn / get-woken-when-it-finishes loop -- that is a
persistent harness, not `claude -p`. So "many tasks" means
*synchronously, inside the turn*. The moment your model needs to watch
async work and react to it across a turn boundary, that is orchestration
-- a skill, or an external driver firing N roba invocations -- and a
different tool. In that picture roba is the worker, not the orchestrator.

### Work that must outlive the caller

A run owns nothing once it returns -- any sub-work still in flight dies
with it, and nothing resumes on its own. So there are two shapes for
work, and only two: **synchronous units** (each unit finishes inside the
turn and the caller loops), or **detached-with-handle** when the
hand-off itself is the point.

The detached form, primary:

```bash
id=$(roba --detach --profile worker -C <dir> -f task.md --trace /tmp/t.jsonl)
roba show "$id" --wait --timeout 600
```

The minted session handle is the only thing on stdout, so `id=$(...)`
captures it. roba verifies the claude binary resolves before detaching,
so it refuses rather than print a handle for a dead-on-arrival child. A
piped stdin that carries data is refused rather than silently lost (the
detached child's stdin is `/dev/null`; this data check is unix-only for
now). Nobody is watching the run, so pair it with the rails
(`--max-turns`, `--max-budget-usd`).

`show --wait` reports the detached run's real outcome, not a
reconstruction. The detached child writes a **run receipt** at its exit
seam (`$ROBA_STATE_DIR/runs/<id>.json`, else `$XDG_STATE_HOME/roba/runs`,
else `~/.local/state/roba/runs`) carrying its typed exit code, and `show`
prefers it over the `stop_reason` heuristic: `.result.is_error` is true
on a failure, `.result.exit_code` carries the code, and `show` itself
exits with that same code. So `roba show "$id" --wait || handle_failure`
works, and a run that died before claude persisted a session is reported
rather than waited out. A receipt is written only for runs roba detached;
without one, `show` behaves exactly as before (`is_error: false`, no
`exit_code`, exit 0). The terminal receipt also records the run's
observed spend (`cost_usd`, from claude's own result event; absent means
unknown, never zero), and receipts expire: records older than ~30 days
are swept when a new detached run starts.

Two derived views make a batch of detached runs legible without a
resident process:

```bash
roba jobs                 # ps-like table over the receipts: session, honest
                          #   state (ok / exit N / running / stale?), cost, age
roba watch                # wait on every running detached run; one line per
                          #   completion, OSC-9 terminal bell; 0 all ok /
                          #   1 any failed / 4 timeout
roba watch 43a14535 9f2b  # or watch specific runs (unique id prefixes ok)
```

`stale?` is the honest column: a `running` record whose pid is gone --
the run was killed too hard to record its exit. `roba jobs --json` wears
the same `{ "version": 1, "result": ... }` envelope as every other
report verb.

The manual fallback (older versions, or no `--detach`):

```bash
nohup roba --session-id "$(uuidgen)" -C <dir> -f task.md >/dev/null 2>&1 &
roba show <id> --wait
```

Never bare fire-and-forget: an orphaned branch and an empty draft PR are
the signature of that failure.

The orchestrator side of the same rule: if you orchestrate *from inside*
`roba -p`, you get exactly ONE turn. When the model stops calling tools
and writes its final response, the process exits -- there is no
re-invocation and no cross-turn background-completion notification (that
is a persistent-harness feature, not a `-p` one). So either block
in-foreground (`roba show <id> --wait`) or hand the session handle back
to your caller explicitly; never background a task and stop expecting to
auto-resume. roba injects this as a system-prompt advisory by default
(disable with `--no-agent-notice`, replace with `--agent-notice`).

Address a session by a STABLE handle, not `-c`. `roba -c` continues the
*most recent* session in the project, which silently drifts when an
orchestrator spawns its own roba sub-invocations (a detached worker, a
`roba show`, a `roba profile init`) in the same directory -- the newest
one out-ranks the session you meant to resume. Pin it instead: pass the
short session id (roba resolves a unique prefix) or a named `[session]`
handle from your `roba.toml`.

> [!NOTE]
> As of 2026-06-15 Anthropic meters programmatic usage (claude -p / Agent SDK) separately from interactive Claude. Every roba call is programmatic by construction, so all roba usage -- and the figures `roba cost` reports -- draws from that programmatic allotment, not your interactive limit.

## Bring your own skills and agents

roba is a pure mechanical wrapper -- no bundled skill or agent library.
Drop skills into `~/.claude/skills/` and agents into `~/.claude/agents/`;
Claude Code auto-discovers them. [joshrotenberg/agent-tools](https://github.com/joshrotenberg/agent-tools)
is one curated set if you want a starting point.

## Status

Published on crates.io. The CLI surface (flag names, exit codes, config
schema, `--json` envelope) is intended to be stable across `0.x`; the
library API (`roba::*`, for integration testing) may shift between minor
versions.

## License

MIT OR Apache-2.0
