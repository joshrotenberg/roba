# Running Roba: from one turn to a hot agent

Roba has two operator interfaces over the same agent model:

- `roba run` configures one agent, sends one prompt, waits for settlement, and
  prints the result. This is the CLI-operated path.
- `roba serve` configures one agent and exposes its control contract over MCP.
  The process starts idle and remains hot until shutdown. This is the
  MCP-operated path.

Both use the same provider adapters, finite-run core, permission postures,
session handles, optional services, and typed failures. The difference is who
drives the agent after construction.

## Configure both interfaces once

A versioned `roba.toml` can pin the provider-neutral startup template shared
by `run` and `serve`:

```bash
# Minimal read-only configuration with provider-native ambient context.
roba init

# Opt into shipped managed context by stable catalog ID.
roba init --agent-role roba.repo-worker --prompt roba.issue-worker
```

`roba init --dry-run` prints the exact validated TOML without creating a file.
Initialization refuses recognized sibling configs instead of overwriting them.

```toml
version = 1

[agent]
provider = "codex"
instructions = ["Work in small, reviewable steps."]

[execution]
permissions = "read_only"
timeout_secs = 900

[session]
mode = "sticky"

[context]
ambient_policy = "controlled"
agent = "roba.repo-worker"
prompts = ["roba.issue-worker"]

[extensions.git]
enabled = true
progress_interval_secs = 5
```

With that file in the repository, the smallest commands become `roba run
"Summarize this repository"` and `mcp-repl -- roba serve`. CLI flags override
file scalars and append explicit instruction or context entries. Inspect the
resolved values and their sources with `roba config effective`.

Before asking a model to tune configuration, inspect Roba's bounded project
evidence with `roba config survey --json`. The versioned packet includes safe
effective startup state and a fixed nonrecursive marker inventory, but no file
bodies. The command starts no provider and writes nothing.

To ask a provider for a conservative preview, use `roba config propose
--provider codex`. Roba gives a fresh read-only operation the exact survey as
mandatory MCP context, accepts only one typed proposal, validates built-in
catalog references, and renders the TOML itself. It never writes the preview.
Use `--json` when you also need rationale and mechanical context-read evidence.

Managed context selection is optional. The startup host validates selected
agent, skill, and prompt IDs and reports body-free provenance and fingerprints.
Selected prompts appear in MCP prompt discovery. The selected agent and skills
remain outside provider prompt text and are acquired through generation-fenced
context tools. `roba://context/catalog` inventories availability and selection;
provider read evidence remains separately visible in `roba://context`.

Ambient context is an independent provider-launch policy. The default
`ambient` mode retains provider-native user and workspace discovery.
`controlled` applies a tested provider-specific reduction; inspect
`roba://context` for the resulting retained, suppressed, and unobservable
source classes. `hermetic` currently refuses for Claude and Codex because
neither adapter can prove the provider baseline and managed policy absent.

## 1. One read-only CLI run

The smallest useful Roba invocation is one finite turn:

```bash
roba run --provider codex \
  "Summarize this repository without modifying it."
```

Roba starts a read-only provider run, prints the final answer to stdout, and
exits. Use `--provider claude` to select Claude Code instead.

## 2. Add explicit instructions, context, and a deadline

Instructions define agent behavior. Context supplies facts for the run. Both
flags are repeatable and preserve their command-line order.

```bash
roba run --provider claude \
  --instruction "Be concise and cite relevant file paths." \
  --instruction "Do not modify the workspace." \
  --context "The public API is the primary compatibility boundary." \
  --timeout 300 \
  "Review the error-handling architecture."
```

Unsupported provider controls fail before launch rather than being silently
ignored.

## 3. Work in a specific repository

`-C` changes the effective workspace before Roba constructs the agent. `--git`
adds the typed Git service, while `--writable` grants workspace-write authority.

```bash
roba -C /path/to/repo run \
  --provider claude \
  --git \
  --writable \
  --timeout 600 \
  "Fix the smallest coherent issue, run relevant tests, and summarize the diff."
```

The Git service exposes repository observation to the provider and operator.
Its mutating `git.stage_all` operation remains operator-only.
While a turn is active, `roba://git/progress` caches an operation baseline,
periodic observations, and a final settled snapshot. Set
`progress_interval_secs = 0` to disable periodic polling for a large repository
without losing the baseline and final evidence.

## 4. Consume a typed CLI result

`--json` emits a versioned terminal `RunSnapshot` instead of answer-only text.

```bash
result=$(roba run --provider codex --json \
  "Return a concise architecture summary.")

printf '%s' "$result" | jq '.result.state'
printf '%s' "$result" | jq -r '.result.last_outcome.output'
```

Runtime failures use typed exit codes and a versioned error envelope on stderr.
A failed admitted run may also have a terminal snapshot on stdout, preserving
the work and provider evidence observed before failure.

## 5. Resume a provider session from the CLI

Successful results may contain an opaque provider session handle. Feed its id
back to the same provider with `--resume`:

```bash
first=$(roba run --provider codex --json \
  "Inspect the parser and propose one focused improvement.")
thread=$(printf '%s' "$first" | jq -r \
  '.result.last_outcome.session.id')

roba run --provider codex --resume "$thread" \
  "Implement that improvement and run its focused tests."
```

Roba fences the handle to the selected provider. The provider owns the actual
conversation state; Roba retains only the opaque identity and terminal
evidence it reports.

## 6. Start one hot MCP agent

`roba serve` accepts the same fixed agent configuration as `roba run`, but no
prompt. stdin and stdout are MCP wire data from the first byte.

```bash
mcp-repl --protocol final -- \
  roba -C /path/to/repo serve --provider codex --git
```

Inside `mcp-repl`, the interaction is MCP-only:

```text
agent.turn text="Inspect this repository and summarize its current state."
read roba://agent
read roba://context
read roba://events
agent.shutdown
```

The logical agent stays hot across turns and retains validated provider session
continuity. Each turn still creates and settles one finite provider process.

## 7. Run long work as an MCP Task

The final MCP protocol can return a Task immediately while work continues.
This keeps the same client free for observation and control.

```text
agent.turn text="Run the full test suite and investigate any failures." &
jobs
read roba://agent
read roba://events
wait last --timeout 900
```

Cancelling the Task targets its exact admitted operation and drains provider
work before the Task settles:

```text
cancel last
wait last
```

Task cancellation does not shut down the logical agent. A later `agent.turn`
can reuse it.

## 8. Observe, follow up, or interrupt an active operation

`roba://agent` reports the current operation id. Controls require that id so a
delayed command cannot affect a later turn.

```text
read roba://agent
agent.follow_up operation_id=1 text="Focus only on the failing integration test."
agent.interrupt operation_id=1
```

While the operation is active, the agent resource also reports normalized
provider activity, observation health, elapsed time, and timeout remaining.
Task-aware clients may render the same command/file/MCP/web/plan activity as
live `roba.activity` log notifications. If they do not, repeatedly reading
`roba://agent` and cursor-paging `roba://events` gives the same bounded factual
evidence. `unknown` means the provider has supplied no usable evidence; Roba
does not manufacture a progress percentage.

Follow-up queues another prompt for the next resumed-provider boundary; it
does not mutate the in-flight provider prompt. Interruption cancels and drains
only that operation, leaving the hot agent reusable.

`agent.turn` also accepts optional one-operation overrides. They apply to the
initial provider turn and every queued follow-up in that operation, then the
host returns to its configured defaults:

```text
agent.turn text="Investigate the failing tests." overrides={"model":"provider-model-id","effort":"high","limits":{"timeout_secs":900}}
```

## 9. Inspect and rotate provider-session generations

`roba://agent` reports the provider-neutral session policy, current generation,
continuity availability, and actual core provider turns observed in that
generation. It never exposes the opaque provider session id.

The default `sticky` mode retains validated continuity. `fresh` starts every
operation without continuity, while the current `managed` phase behaves like
sticky retention until the operator rotates it explicitly:

```text
read roba://agent
agent.session.rotate expected_generation=1 strategy="clean"
agent.turn text="Continue with a clean provider context."
```

Rotation is accepted only while idle. The expected generation prevents a
delayed client from discarding newer state. Clean rotation performs no model
call or summary; automatic triggers and summary rollover remain future work.

Configure the lifetime policy with `[session].mode` or `--session-mode`.
`--resume` may seed `sticky` or `managed`, but conflicts with `fresh`.

## 10. Inspect Roba-declared context over MCP

The manifest inventories context without copying its bodies into diagnostics:

```text
read roba://context
read "roba://context/entry?id=agent.instruction.1&generation=1"
```

The provider-facing projection offers the equivalent read-only
`context.manifest` and `context.read` tools. Successful provider reads are
recorded against the exact operation id and context generation. That evidence
proves the MCP request occurred; it does not prove the model understood or
followed the material.

During an active or latest settled operation, the same snapshot includes the
typed launch bootstrap. It shows the exact operation, provider, authority,
manifest fingerprint, and mandatory MCP acquisitions without exposing context
bodies:

```text
read roba://context
```

The provider receives a compact rendering of that contract before its current
goal. The bootstrap points to MCP; it does not concatenate the referenced
material into every turn.

## 11. Combine CLI construction with MCP operation

This is the most complete currently shipped composition:

```bash
mcp-repl --protocol final -- \
  roba -C /path/to/repo serve \
    --provider claude \
    --model sonnet \
    --instruction "Work in small, reviewable steps." \
    --context "Tests are the acceptance boundary." \
    --git \
    --writable \
    --timeout 600
```

The CLI fixes provider, workspace, authority, context, services, and per-turn
limits for the lifetime of the process. MCP then supplies turns and controls,
observes state and events, and ends the agent with `agent.shutdown`.

The provider receives a separate least-authority MCP projection for each
finite operation. It can identify its Roba operation, inspect declared context,
and use explicitly installed provider capabilities such as `git.snapshot`; it
cannot admit turns, steer itself, interrupt the operator, or call shutdown.

## 12. Embed the MCP agent without the CLI

Rust hosts can construct an `AgentInstance` from a suspended `RunSpec` and
serve or connect to its control router directly. `ChannelTransport` provides
an in-process MCP client; `StdioBinding` exposes the same contract over stdio.

An embedding can also start with `ContextPlan::builder_from_run_spec`, add
MCP-native context with an explicit operator/provider audience and declared
precedence, and pass the immutable plan to
`AgentInstance::new_with_context_plan`. Roba rejects a plan that hides or
changes context already compiled into the executable `RunSpec`.

This path is MCP-only at the application boundary: the embedding owns process
lifetime and configuration instead of invoking the `roba` binary. It is the
right base for a scheduler, desktop application, test harness, or a parent Roba
that intentionally manages another Roba.

## Choosing a level

| Need | Start with |
| --- | --- |
| One answer or edit | `roba run` |
| Scripted terminal evidence | `roba run --json` |
| One explicit follow-up | `roba run --resume` |
| Several sequential turns | `roba serve` + MCP client |
| Long work plus observation | MCP Task on `agent.turn` |
| Follow-up work or cancellation | `agent.follow_up` / `agent.interrupt` |
| Explicit provider-context reset | `agent.session.rotate` |
| Repository-aware observation | `--git` |
| Custom host or scheduler | `AgentInstance` + MCP binding |

## Not shipped yet

The current contract makes these compositions possible, but Roba does not yet
ship them as base behavior:

- schedules that periodically call `agent.turn`;
- queues or retry policy above single-flight admission;
- parent/child Roba orchestration;
- context acknowledgement gates or automatic session rotation and summaries;
- Unix-socket or authenticated operator HTTP bindings;
- aggregate budgets spanning several finite turns.

Those belong in higher layers or explicit extensions. The base remains one
logical agent, one active operation at a time, and one MCP contract.
