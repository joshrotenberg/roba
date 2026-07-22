> Status: research record (2026-07-22). The build spec distilled from
> this research lives at joshrotenberg/orario (docs/SPEC.md and
> docs/PRIOR-ART.md); this document keeps the fuller landscape study,
> the in-house context, and the decision trail.

# A mechanical, MCP-enabled cron server for agent runs

Research notes, 2026-07-22. Not a build plan. The question: the server idea
(roba serve, duplex-per-process) never gained traction because a single-process
run covers almost every use case -- but *scheduled* runs have proven repeatedly
useful. What is the ideal scheduling harness for roba-shaped agent runs, does
it exist anywhere, and what would we put together if we built it (in Rust)?

The charter, one sentence (Josh, 2026-07-22): **"run roba with these params
on this schedule."** That sentence is the scope discipline, the same job
"sharp sugar over `claude -p`, that and that alone" does for roba --
everything below either serves it or gets cut.

The expanded target: **Oban plus its Cron plugin, reimagined as a Rust
single binary whose control plane is MCP** -- a purely mechanical daemon (no
LLM in its own loop) that fires, caps, records, and reports roba runs on a
schedule, and lets both humans and agents add/edit/inspect schedules at
runtime through MCP tools.

## 1. Why not just cron/launchd (the short version)

We already do this: `com.josh.roba-stars.plist` fires `roba stars --quiet`
via launchd today, and issue #390 (the roba-cron design of record) is a whole
fleet design built on launchd plists + shell glue + "GitHub is the database."
It works. The gaps that keep resurfacing:

- **Observability is DIY.** launchd gives you an exit status in a log if you
  wire it up. Every job re-invents spend tracking, receipts, "did it even
  fire," and staleness detection.
- **A missed run is silent, and a duplicate run costs money.** Agent runs are
  *paid*; overlap prevention and uniqueness matter more than for normal cron
  jobs, and dead-man detection ("this stopped happening") matters as much as
  failure detection.
- **Dynamic scheduling is crontab editing.** There is no story for "an agent
  (or a human mid-conversation) adds a routine, previews it, edits it" --
  which the MCP surface makes natural.
- **Laptop semantics.** The deploy target is a laptop, not a server: sleep,
  catch-up policy, and jitter are first-class concerns (anacron semantics,
  which plain cron lacks and launchd only partially covers).

(The external research below tests how much of this launchd/systemd actually
do cover, honestly.)

### The generic "modern cron" checklist, as a foil

The infra world's standard next-gen-cron feature list (this version via a
Gemini session, but it is the consensus list) is: (1) native alerting +
structured/JSON logging + webhook push, (2) distributed execution with
exactly-once locking, (3) ephemeral container isolation per job, (4)
second-level precision + declarative retry/backoff policies.

Held against *agent* runs on a *laptop*, it splits cleanly:

| Generic feature | Agent-run translation | Verdict |
|---|---|---|
| Structured logging + alerting | Run receipts already ARE the structured record (#441); the scheduler adds the push half (notify on failure, on missed window, on dead-man) | **Keep -- and half is built** |
| Declarative retries/backoff | Keep, but *typed*: roba's exit taxonomy means retry policy can branch on 5/7 (cap hit) vs 2 (auth -- never retry) vs 4 (timeout) | **Keep, sharpened** |
| Exactly-once / distributed locking | One laptop, no cluster -- but the *motivation* transfers with more force: a duplicate fire costs dollars, not just data races. Local uniqueness (flock lanes, #444 slice 4) is essential; Raft is not | **Keep the property, refuse the machinery** |
| Ephemeral container isolation | Isolation already lives at the roba layer: worktrees (`-w`), hermetic bundles (`--hermetic`/`--bundle`), permission postures. The scheduler fires verbs; it does not own sandboxing | **Refuse -- wrong layer** |
| Second/millisecond precision | Agent runs take minutes and cost dollars; minute-level cron is already finer than the economics support | **Refuse** |

The divergence is the design insight: generic modern cron optimizes for
*fleet throughput*; an agent scheduler optimizes for *spend legibility and
silence detection* on a machine that sleeps.

## 2. The in-house landscape (what already exists, ours)

This idea does not land in a vacuum. Inventory, so the new thing is defined
by its *boundaries* against these:

| Project | Language | What it is | Relation to this idea |
|---|---|---|---|
| **custode** (genagent) | Elixir, Oban+SQLite | The operated fleet: routines.toml roster, cron ticks via `ObanClaude.Agent.Tick`, per-routine profile/workspace/budget, gates/approvals, fleet dashboard, advisors, MCP verbs (`list/add/preview/update/remove_routine`, `list_gates`) | **The existing realization.** Custode IS a scheduled, observable, MCP-pokable agent fleet. It is the feature bar any Rust equivalent must justify itself against. |
| **oban_claude** (genagent) | Elixir | `Oban.Worker` over claude_wrapper + the `Agent` layer (state machine, `if_offline: "start"`) | The library custode stands on -- the "Oban gives you the contracts free" thesis, proven. |
| **telaio** | Rust | Repo-as-state-machine daemon (GraphQL snapshot per tick, mechanical transitions, dispatches claude where judgment is needed). Canonical model doc: `docs/agentic-repo-automation.md` | A *reconciler*, not a scheduler: its tick needs something to fire it. The cron server is one layer below telaio, not a rival. |
| **ordito** | Elixir, Oban | telaio's Oban-native sibling | Same layer as telaio; Oban already gives it the tick. |
| **spola** | Rust | One-process one-thread tick executor over GitHub state, JSONL ledger, dispatches roba runs | Same reconciler layer. spola's tick is `once/run`; a scheduler would *fire* spola. |
| **roba #390** | design doc | The launchd + shell + "GitHub is the database" fleet design | The no-daemon baseline this idea deliberately upgrades: same protocol layer, different firing/observability substrate. |

The clean layering that falls out:

```
judgment       claude (via roba)            -- the only LLM in the system
reconcilers    telaio / spola / ordito      -- repo state machines, per-tick
verbs          roba aliases + profiles       -- one wide shot each
runner         roba                          -- envelope, caps, exit codes, receipts
scheduler      << the gap this idea fills >> -- fires ticks and verbs, records outcomes
os             cron/launchd/systemd          -- what the scheduler replaces/wraps
```

Custode currently spans the scheduler + reconciler layers (Oban is both its
queue and its cron). The roba-side question is whether a *standalone
mechanical scheduler* -- decoupled from any one reconciler, speaking MCP --
earns its place in Rust.

## 3. The roba substrate (what the scheduler would stand on)

The recent roba work is, in hindsight, exactly the substrate a mechanical
scheduler wants. None of it needs to change:

- **Run receipts (#441, shipped).** Every detached run writes a durable
  `{session_id, pid, state, exit_code, started_at, ended_at}` record to
  `$XDG_STATE_HOME/roba/runs/`, single-writer, pid-ownership-guarded. A
  scheduler's "what happened" is a file read, not log parsing.
- **Typed exit codes (0-7).** A scheduler can *branch mechanically*: 5/7 =
  recoverable cap hit (re-fire with a bigger envelope or just note it), 2 =
  auth (alert, stop firing), 4 = timeout, 6 = no usable output. This is the
  agent-tailored part generic cron cannot have.
- **`--detach` + `show --wait`.** Fire-and-record with a handle already works.
- **Profiles/aliases as the unit of scheduling.** A routine is "fire this
  verb with this posture" -- `roba stars --quiet`, `roba issue-workflow 123`.
  The scheduler never composes prompts; it fires named, config-linted verbs.
  (Custode's routines.toml made the same call: `profile = "backlog_worker"`.)
- **The #444 refusal list is the doctrinal handoff.** roba pre-committed that
  retry, reaping, chain graphs, and "pending-but-unfired intent" are exactly
  the runtime state roba refuses to own and that "belong to a separate
  harness if they ever become real." This is that harness. The scope line
  stays intact: roba is the worker; the scheduler is a different tool that
  *calls* roba and reads only roba's public artifacts (receipts, exit codes),
  never `.claude/` internals.

## 4. External prior art

Six research lenses plus an adversarial pass that specifically tried to
falsify "the intersection is unclaimed." Condensed; every claim carries a
source. Checked 2026-07-22.

### 4.1 The verdict, stated carefully

The six-axis intersection (durable + mechanical + MCP-native + agent-aware +
Rust + local-first) has been **attempted but has no viable occupant**. The
adversarial pass found near-misses the first sweep missed, so the honest
phrasing is not "nothing exists" but:

> Every occupant of the niche is dead, deprecated, archived, or sub-5-star
> with no paid-run semantics. The **paid-run-governance layer** -- typed
> exit-code policy, budget ledger, receipt-native observability -- is
> uncovered anywhere, by anyone.

The near-misses, which any design doc must cite:

- [JheisonMB/task-trigger-mcp](https://github.com/JheisonMB/task-trigger-mcp)
  -- the closest: a **single Rust binary MCP server** (cron + file-watch
  triggers, 12 tools, SQLite persistence, streamable HTTP) that execs local
  headless agent CLIs. All six axes. But: 1 star, no paid-run semantics
  (no budget caps, no exit taxonomy, no receipts), and **deprecated** --
  confirmed by the deep-dive with a twist. The README looks active (which
  briefly fooled us into retracting the claim), but the crates.io v1.4.0
  description reads "DEPRECATED: use agent-canopy instead" and `main.rs` at
  HEAD is a 6-line stub that prints the deprecation and exits 1. The named
  successor does not exist on crates.io; the real one is the GitHub-only
  `UniverLab/harness-canopy`, an "agent operations center" that scope-crept
  out of the sharp-scheduler niche entirely. **The only Rust occupant
  abandoned the niche for a platform** -- the strongest external validation
  that the sharp, refusal-driven, laptop-first version is unbuilt territory.
- [frr149/claude-cron](https://github.com/frr149/claude-cron) -- TS/Bun,
  SQLite WAL, local Claude CLI exec, per-task budget limits, NL schedules.
  Everything except Rust. **Archived read-only April 2026, 0 stars.**
- [jshiv/cronicle](https://github.com/jshiv/cronicle) -- Go, active (v0.7.1,
  23 stars): SQLite queue, **per-run `budget_usd` caps that abort runs**,
  per-run transcripts, DAG deps. But it embeds its own API-key agent loop
  (not exec-a-harness) and is an MCP *client*, not MCP-controllable.
- [kylemclaren/claude-tasks](https://github.com/kylemclaren/claude-tasks) --
  Go TUI, execs local claude CLI on cron, SQLite, **usage-threshold
  auto-skip** (skips fires when rate-limit consumption is high). No MCP.
- **Claude Code Desktop scheduled tasks** -- the vendor's local tier the
  first sweep underweighted: runs on your machine, persists, local file
  access, per-task MCP config, 1-minute floor, with an MCP tool surface
  ([docs](https://code.claude.com/docs/en/scheduled-tasks)). Absorbs much of
  the everyday "schedule a claude run locally" demand. Not mechanical (it IS
  claude), needs the Desktop app resident, no exit-code policy, no
  budget/receipt ledger, no firing of arbitrary local binaries as policy.

The **dead-project pattern is itself a datum**: effectum (stale, 0 reverse
deps), claude-cron (archived), task-trigger-mcp (deprecated at the crate
level, 108 lifetime downloads, successor scope-crept into a platform) --
three independent attempts died with near-zero adoption. The niche is real
but tiny. That is fine for a tool built for ourselves (roba's own model),
fatal for a tool built for a market.

### 4.2 Rust substrate (build-on-what)

No maintained Rust crate is already "durable cron for local subprocess
jobs." The field splits cleanly: everything durable-and-maintained is
Postgres server software (graphile_worker, underway, fang, sqlxmq);
everything local-and-light is an in-memory timer wheel that forgets on
restart (tokio-cron-scheduler sans PG/NATS, delay_timer, clokwerk, sacs).

- **apalis 1.0.0-rc.9** (+apalis-sqlite) is the one near-Oban candidate:
  heartbeats + dead-worker re-enqueue (Lifeline equivalent), durable-before-
  processing, retries/backoff, tower-native (synergy with tower-mcp). But:
  schedules are **code-bound** (`CronStream` built into a worker -- no
  runtime "add a schedule at 2pm" API), no Oban-style uniqueness, still an
  rc, and it assumes jobs are Rust closures. For a subprocess-firing daemon
  most of its value is dead weight. **Watch 1.0; do not adopt at rc.**
- **effectum** proved "Oban-lite on SQLite" fits in one small crate -- and
  then died (0 reverse deps, stale 2y). Encouragement about surface area,
  warning about who maintains it.
- The composed substrate that fits: **croner** (active, Quartz-ish `L`/`#`/
  `W` support) or **cron** (17.8M downloads, the default) for next-fire
  math + **SQLite** (WAL) with a hand-rolled Oban-shaped claim/reap/prune
  core (~a few hundred lines you fully control) + optionally
  **english-to-cron** as an MCP-tool nicety ("every weekday at 9am" from an
  agent). The scheduling math is commodity; the value is everything around
  the fire.

### 4.3 MCP cron as a category

Roughly a dozen projects, dominated by single-author stale demos in
Go/TS/Python. Flagship: [jolks/mcp-cron](https://github.com/jolks/mcp-cron)
(Go, SQLite, 12 tools, stdio + streamable HTTP, `--prevent-sleep` for
laptops, and one genuinely good move: a **read-only SQL query tool over run
results** as the observability surface). Most production-shaped:
[CronLite](https://github.com/djlord-it/cronlite) -- literally the Oban
pattern ported to Go (Postgres, SKIP LOCKED, leader election, reconciler) --
but it fires **HTTP webhooks only** (cannot exec a local run), needs
Postgres, AGPL, 1 week old. Anthropic's MCP connector registry has **zero**
cron/scheduler entries. Design reads worth stealing regardless of adoption:
[claudecron](https://github.com/phildougherty/claudecron)'s trigger taxonomy
(cron + interval + file-watch + hook events + dependency DAG + manual) and
jolks' SQL-over-results tool.

### 4.4 What to steal from Oban (graded by the in-house realizations)

The meta-lesson from oban_claude/custode: **keep Oban's contracts, discard
Oban's distribution.** Single process + one SQLite WAL transaction gives the
same at-most-once claim Oban gets from `SKIP LOCKED` (Oban's own Lite engine
makes the identical concession). Graded:

| Contract | Grade for a single-user local agent scheduler |
|---|---|
| At-most-once dispatch per beat | ESSENTIAL contract, trivial mechanism (one SQLite txn + a `last_fired` minute map) |
| Retry/backoff engine | Mostly INVERTED: paid runs make most failures cancel-not-retry. Steal oban_claude's **outcome classifier** (retry only timeout/rate-limit, bounded), not the engine. Ticks are `max_attempts: 1` -- a missed beat is missed, never replayed stale |
| Overlap policy | ESSENTIAL and Oban does NOT provide it (it stacks). The correct contract is oban_claude's `if_busy: skip \| queue`, default **skip** -- "a missed beat is better than a backlog of stale scheduled prompts" |
| Lifeline (reaping) | ESSENTIAL in reduced form: boot-time reconcile of runs marked in-flight when the process died |
| Timezone-aware cron | ESSENTIAL (custode learned it twice); read tz every tick so it is live-editable |
| DynamicCron (runtime CRUD) | ESSENTIAL -- and custode rebuilt it on OSS with the design to copy: a minute-tick loop that **re-reads the roster file every tick**, evaluates cron itself, dedupes via `last_fired`. Edits live at the next minute, no restart |
| `if_offline: "start"` | **The single best idea to port**: the schedule entry carries the full start config, so the crontab cold-starts and *revives* its agent after any restart. The crontab IS the agent spec |
| Uniqueness, Pruner, dashboard, leadership | LATER / SKIP (single node; GC eventually; status command before any UI) |

Custode's spine, distilled by the research: *a tz-aware minute loop over a
live-reloaded TOML roster whose entry is the whole agent spec, firing
at-most-once skip-on-busy beats recorded in a durable SQLite run-and-spend
ledger with daily budget auto-pause and boot-time orphan reconcile.*
Everything else (gates, sensors, advisors, dashboards) layers on that.

### 4.5 OS incumbents: what they cover, where the wrapper breaks

launchd and systemd already fire commands at times competently -- and the
daemon should be *supervised by* them (KeepAlive / `Restart=`), never
reinvent process supervision. The honest table: launchd catches up after
**sleep** (coalesced) but not power-off, is singleton-per-label, has no
jitter, and its calendar semantics have drifted across macOS releases;
systemd timers are the strongest incumbent (`Persistent=`,
`RandomizedDelaySec=`, rich `OnCalendar=`) but are Linux-only and their
dynamic form (`systemd-run`) does not survive reboot. fcron's `fcrondyn` is
the Unix lineage's closest ancestor to a dynamic scheduling API.

The gap is not firing -- it is everything around the fire: paid-run
uniqueness across submission paths (a coalesced catch-up plus a manual kick
= two billed runs), per-exit-code policy (no incumbent can express "on 7
re-fire once with a higher cap; on 2 halt everything and alert"),
receipt-native observability ("what did the last 7 nightly runs cost, which
sessions did they mint"), dead-man detection (a run that stops *happening*
is invisible to the scheduler that failed to fire it -- why healthchecks.io
exists), and durable runtime scheduling with an API. The breakdown point:
**the wrapper script accretes the product** -- flock + receipt parsing +
exit-code branching + cost ledger + heartbeat pings as untested bash, times
N routines, times two platforms. That accreted wrapper is the custom
daemon, just worse.

### 4.6 Durable-execution engines: no fit, one spike

Every serious contender is a platform (Temporal, Windmill, Hatchet, Kestra,
trigger.dev), inverts invocation through a resident service the engine
calls (Restate, Inngest), or is cloud-bound (val.town, GitHub Actions --
whose scheduler is documented-unreliable: routine 5-30 min delays, silent
drops under load, auto-disable after 60 days of repo inactivity). None of
the ten has an MCP interface, so adopting any engine still means writing
the differentiated layer anyway.

The one genuine near-fit: **[Obelisk](https://obeli.sk/features/)** --
single Rust binary, embedded SQLite, built-in cron, and Exec activities
that run arbitrary local executables as durable, retried, observed jobs.
Nearly "cron with a database" verbatim; it could exec `roba --json` today.
Against it: pre-release (schema-change warnings, ~single maintainer), AGPL
engine, gRPC/web control (not MCP), and its core WASM-determinism workflow
layer is machinery this use case never touches. **Worth a one-day spike as
buy-vs-build calibration only.** Steal Temporal's Schedules *spec* (overlap
policy, catchup window, pause/backfill) as the semantics reference.

### 4.7 Agent scheduling in the wild: the convergent list

Five patterns in the wild (GH Actions cron, cron/launchd + wrapper,
Ralph-style loops, harness-native/vendor cloud, bespoke fleet schedulers).
Every team that outgrew pattern 2 converged on the same five requirements
generic cron demonstrably fails:

1. **Overlap/duplicate-fire prevention** where every duplicate burns dollars
2. **Aggregate cost caps + spend telemetry** -- the "doom spiral" insight:
   100 individually-normal calls, hundreds of dollars in aggregate;
   per-request limits cannot see it. Policy: alert-and-let-human-triage,
   never auto-kill in-progress work
3. **Failure visibility**: notify-on-failure with durable per-run receipts,
   silence on success ("tasks would fail silently" is the recurring origin
   story of every bespoke scheduler)
4. **Laptop catch-up semantics** -- nobody in the agent space has
   productized anacron semantics; the showcase 24/7 local agent runs on a
   Mac *Mini*, sidestepping sleep rather than solving it
5. **A security boundary on dynamically-created schedules.** The sharpest
   finding: Hermes Agent
   [#8886](https://github.com/NousResearch/hermes-agent/issues/8886) --
   prompt injection creating a persistent cron job that respawns a
   full-privilege agent on every fire; regex prompt-scanning bypassed 6/7
   times. CrowdStrike treats agent cron entries as a persistence mechanism
   to purge. **Direct design consequence: MCP-created schedules must only
   reference pre-declared, config-linted verbs (profiles/aliases), never
   free-text prompts; write-shaped routines gate on approval.**

The counter-position (cron + wrapper is enough; "reliable boring systems
beat dramatic autonomous loops") holds right up until: the machine sleeps,
job count grows past a handful, you need aggregate spend visibility, or an
agent needs to schedule dynamically -- the four points where every source
that started with cron ended up building the scheduler.

## 5. What we might put together

Verdict first: **building is defensible, and small.** The scheduling math is
commodity (croner + SQLite); no existing tool covers the paid-run-governance
layer; and the differentiated core is precisely the part that is
roba-specific (exit taxonomy, receipts, profiles/aliases, config lint). The
research also says what NOT to build, loudly.

### Ruling: roba-native (decided 2026-07-22)

Not a generic agent scheduler -- a **roba-family sibling binary** (the
roba-server precedent: workspace member consuming roba-types/roba-core,
never a daemon inside the roba CLI). The research makes the argument, Josh
confirmed it ("that's what we really want it for anyway"):

1. **Generic is the graveyard.** The dead prior attempts were all generic;
   Anthropic's Desktop tasks absorb the generic local demand. The uncovered
   layer -- paid-run governance -- is definitionally roba's ABI.
2. **Security falls out of the substrate.** "Dynamic schedules reference
   pre-declared reduced-privilege definitions only" (the Hermes #8886
   mitigation) IS roba's alias/profile/config-lint layer.
3. **Concrete implication:** promote the receipt schema from roba's
   `src/receipt.rs` into **roba-types**, making the scheduler the second
   published-contract consumer (after spola). The scheduler reads receipts
   and exit codes through the published contract, never `.claude/` or roba
   internals.

### The mental model (Josh, 2026-07-22)

"The merger of cron and a roba param struct." A routine = (cron expression,
roba run envelope). This is the third instance of roba's recurring move --
alias = params + template, persona = profile + agent, **routine = profile +
cron** -- each time no new primitive, one new dimension on the existing
struct.

The fork inside it: **fat routine** (roster embeds the full ~45-knob
envelope; scheduler owns resolution, duplicating roba's config layering) vs
**thin routine** (roster holds `cron` + a pointer -- `roba -C <dir>
--profile X <verb>` -- and the target repo's own roba.toml pool supplies
the envelope, resolved by roba exactly as today). **Leaning thin:** the
scheduler stays a clock that turns rows into argv; `config lint` already
validates the pointed-at layer; the Hermes verbs-only posture is structural
rather than enforced; and custode independently converged on the same shape
(`profile = "backlog_worker"` references, not embedded envelopes).
Per-fire overrides (budget, model) ride the argv, where roba's normal
CLI-wins precedence handles them.

### The shape

One Rust binary. Supervised by launchd/systemd (it does not reinvent
KeepAlive). SQLite (WAL) for the ledger. A tz-aware minute loop over a
live-reloaded TOML roster. Two faces: the loop, and an MCP control plane
(stdio + streamable HTTP, the roba-server pattern).

```toml
# roster.toml -- the schedule entry IS the run spec (custode's lesson)
[[routine]]
id         = "roba-stars"
cron       = "0 9 * * *"
verb       = ["stars", "--quiet"]        # a roba alias -- NEVER free text
dir        = "~/Code/github.com/joshrotenberg/roba"
if_busy    = "skip"                      # skip | queue; skip is default
catch_up   = "once"                      # none | once  (anacron semantics)
on_exit.2  = "halt-all"                  # auth failure: stop the world
on_exit.7  = "note"                      # budget cap: record, do not retry
daily_budget_usd = 5.0                   # aggregate rail, auto-pause at cap
```

- **Fire = spawn `roba <verb>`** in its own process group with a wall-clock
  timeout; the run's record is roba's own receipt (#441) joined to the
  scheduler's row (routine id, fire reason: cron|manual|catch-up, outcome).
  The scheduler reads receipts and claude-visible exit codes only -- never
  `.claude/` internals. Same scope line as roba's.
- **Typed exit-code policy table** -- the feature nobody anywhere has: 2
  (auth) halts every routine and alerts (each further fire fails
  identically, for money); 5/7 (caps) are recorded, optionally one
  re-fire with a larger envelope; 4 (timeout) is the only default retry,
  bounded. This is oban_claude's classifier, made declarative.
- **Spend ledger + dead-man**: one row per fire including failures; daily
  budget auto-pause (resume is human); "expected receipt absent" detection
  natively -- it owns both the schedule and the receipts, so silence is
  visible without healthchecks.io.
- **MCP surface** (custode's verbs, straight port): `list_routines`,
  `preview_routine` (read-only diff), `add/update/remove_routine` (gated
  write-back to the roster file -- the file stays the source of truth),
  plus read-only observability: `runs`, `spend`, `next_fires`, and jolks'
  good idea, a read-only query tool over the ledger. Security posture from
  Hermes #8886: dynamic schedules reference **verbs only**; `roba config
  lint` already validates the verb layer; write-posture routines can gate.
- **Boot reconcile**: runs marked in-flight by a dead scheduler get
  resolved from receipts (the receipt outlives everyone -- that was the
  point of #441).

### The refusal list (as loud as roba's)

No LLM in the daemon's loop, ever. No retry *engine* (a classifier and a
bounded timeout-retry). No distributed anything (no leadership, no cluster).
No container/sandbox layer (roba owns isolation: postures, worktrees,
hermetic bundles). No sub-minute precision. No dashboard in v1 (a `status`
command and the MCP resources; a UI only if dogfooding demands it). No
queueing backlog of stale beats (`if_busy = skip` by default).

### Relationship to the family

It fires roba verbs and telaio/spola ticks; it is the layer below the
reconcilers, above the OS. Custode remains the batteries-included Elixir
fleet (judgment, gates, advisors, dashboard) -- this is deliberately just
the mechanical spine, usable by anything that can put a verb in a TOML
file. If custode ever wants a Rust firing layer, the boundary is already
drawn. Name candidate, keeping the Venetian register: **orario**
(timetable). Note the dogfood log already used "orario" for a test project
(2026-06-17) -- check for collision before claiming it.

### Buy-vs-build calibrations before any code

1. A one-day **Obelisk spike** (exec `roba --json` from its cron; measure
   what the WASM layer costs in friction) -- calibration, not adoption.
2. **Watch apalis 1.0 final**: if it lands uniqueness + a dynamic cron
   source, it becomes substrate rather than rival.
3. Re-read **Claude Desktop scheduled tasks** honestly: for "run a claude
   prompt daily on my machine," the vendor now covers it. The daemon's
   claim is only the mechanical/headless/exit-code-aware/multi-verb layer.

## 6. Build decisions from the source deep-dive (2026-07-22)

A second workflow cloned and read the SOURCE of all 8 near-miss projects
(scheduler loops, executors, store schemas, MCP tool definitions -- not
READMEs), graded every feature STEAL/ADAPT/REJECT/GAP against the charter,
then synthesized. Two individual read grades were overruled in synthesis,
marked (!) with reasons. This section is the build-decision record.

### 6.1 Feature decision matrix

Grouped by rubric axis. Best-implementation project in parentheses. (!) marks grades where reports disagreed.

| Feature | Best prior implementation | Decision | Rationale |
|---|---|---|---|
| **Scheduling** | | | |
| Per-entry IANA tz, compare in-zone, store fire times UTC | cronlite | STEAL | Only project that does tz correctly end-to-end; port verbatim |
| Persist `next_fire` computed by a parser independent of the fire mechanism | claudecron (split) + mcp-cron (store-and-poll) | STEAL | One source for preview, dead-man math, and restart durability |
| Catch-up window from last-tick with cold-start anchor | cronlite | ADAPT | Cap at 1 fire (`catch_up = once`), record skipped occurrences; their 1000-cap is server-shaped |
| One-shot entries with catch-up + auto-disable (`@once`, `expires`) | claude-tasks / obelisk | ADAPT | Roster sugar; claude-tasks proved one-offs are the shape that *should* catch up on wake |
| NL schedule input | cronlite `resolve-schedule` + claude-cron precedence stack | ADAPT | Authoring-time only: deterministic parse, compile to cron, preview next-5, store cron. Never in the loop |
| Sub-minute granularity | mcp-cron, claude-tasks | REJECT | Spec refusal; mcp-cron shows it compounds the overlap gap |
| In-daemon LLM schedule synthesis (`smart_schedule`) | claudecron | REJECT | LLM in the loop, hard refusal; the generate-validate-store shape already lives in roba's config-draft |
| **Triggers** | | | |
| Manual fire = set `next_fire = now`, normal loop picks it up | claude-cron / mcp-cron | STEAL | One execution path; manual is a ledger poke, not a second code path |
| Trigger==cron unification, `trigger_type` on the ledger row | cronicle + cronlite | STEAL | As an invariant: every downstream feature works for both fire sources for free |
| Manual-only entries with mandatory rationale string | claudecron | STEAL | Cheap roster discipline; documents why a verb has no schedule |
| File-watch triggers | claudecron (chokidar semantics) | REJECT v1 | Always-on watchers in the daemon; bank `awaitWriteFinish`+debounce notes if ever added |
| DAG / dependency triggers | cronicle | REJECT | Orchestration policy; claudecron's volatile version shows the failure mode |
| Agent lifecycle hooks | claudecron | REJECT | Push-shaped (2026-06-24 poll-over-push ruling) and their delivery is broken by construction |
| **Execution** | | | |
| Process group (`Setpgid`) + `WaitDelay` SIGINT-then-SIGKILL escalation | cronicle `proc_unix.go` | STEAL | The only correct kill in 8 projects; 3 lines; everyone else orphans grandchildren |
| Wall-clock timeout via context on the spawn | cronicle | STEAL | Paired with the process group; task-trigger-mcp's lease-only "timeout" is the cautionary tale |
| Env allowlist per roster entry | obelisk `env_vars` | STEAL | Declare inherited vars instead of leaking the daemon's environment |
| Context env block for the child (`ROBA_RUN_ID`, trigger type...) | claudecron | STEAL | Maps directly onto roba's env surface |
| stdout=result / stderr=logged / exit=classification seam | obelisk | STEAL | Byte-identical to roba's existing contract; nothing to invent |
| Buffer-everything, no-kill executors | ttm / claude-tasks / claude-cron | REJECT | The universal defect; confirms process ownership is the hard part |
| **Durability** | | | |
| WAL + `busy_timeout` for two-writer (MCP plane + loop) topology | claude-cron / mcp-cron | STEAL | Validated twice; exactly our process split |
| `UNIQUE (entry, scheduled_at)` natural-key idempotent insert | cronlite | STEAL | Restart, overlapping ticks, and re-emits collapse to silent dup-skip by DDL |
| Terminal-state guard inside the UPDATE (`WHERE status NOT IN ...`) | cronlite | STEAL | No code path can regress a finished row |
| Ledger row column set (trigger ctx, duration, exit_code, cost_usd, tokens) | claudecron `executions` | ADAPT | Near 1:1 with our ledger row; add receipt path + digest, drop output blobs |
| Slim ledger + fat sidecar files (bytes live outside the DB) | cronicle | STEAL | Store the receipt path, never absorb roba's output; kills unbounded DB growth |
| Lazy lease resolution instead of a reaper thread | task-trigger-mcp | ADAPT | Keep the mechanism, attach the kill they never wrote |
| **Overlap** | | | |
| `if_busy skip\|queue` | nobody | GAP-CONFIRMED | Absent in all 8, including the elaborate ones; genuine differentiation |
| Missed-fire receipt naming the blocking run | task-trigger-mcp | STEAL | Makes skip auditable and dead-man math honest |
| Optimistic CAS claim on `next_fire` | mcp-cron `AdvanceNextRun` | STEAL | Fire-claim primitive; crash-safe, gives catch-up collapse for free |
| **Budget** | | | |
| Daily dollar rails + auto-pause | nobody | GAP-CONFIRMED | Zero prior art; cronicle's per-run cap is computed-not-observed |
| OAuth usage-window gate (5h/7d utilization endpoint) | claude-tasks | ADAPT (v2) | The endpoint is the steal; invert to fail-closed, distinct `skipped`, snooze to `resets_at` |
| Computed cost from a hand-maintained price table | cronicle / claude-cron | REJECT | Both drift silently; orario reads observed cost from roba receipts, never computes |
| **Outcomes** | | | |
| Typed exit-code -> declarative policy table | nobody | GAP-CONFIRMED | Every project discards or ignores exit codes; the core differentiator |
| Per-verb circuit breaker as auto-pause | cronlite | ADAPT (v2) | Re-key from URL to verb; open = defer not fail is the subtle correct choice |
| Generic retry-toward-completion engine | obelisk / cronlite / claudecron | REJECT | Paid agent runs are not idempotent; classifier only, exit-4 bounded retry is the whole retry story |
| `skipped` as a distinct terminal status | (claude-tasks anti-lesson) | STEAL the lesson | Their skip-as-failed conflation poisons history and silences the one alert that matters |
| ntfy.sh push channel (single POST, header metadata) | claude-cron | STEAL | 53 lines, phone-reaching, right weight for auth-halt and dead-man alerts |
| Agent self-reported outcomes over MCP | task-trigger-mcp | REJECT | Model-compliance-dependent; the durable receipt is the deterministic version |
| **Observability** | | | |
| Read-only SQL query tool (`PRAGMA query_only` on dedicated conn, row cap, live schema in tool description) | mcp-cron | STEAL | ~70 lines; exactly the ledger-query surface our MCP plane wants |
| Tick-drift measurement | cronlite | STEAL | On a laptop, drift IS sleep detection; feeds the catch-up path |
| `${last_run}` dispatch-time token from the authoritative store | cronicle | STEAL | Highest-value unspecced item; enables "since last run" verbs in ~40 lines |
| Live progress tail via ledger appends | claudecron | REJECT v1 (!) | Report graded STEAL; overruled -- roba `--trace` already owns mid-flight visibility |
| Env redaction (`K=***`) in anything persisted | cronicle | STEAL | Cheap hygiene |
| **MCP surface** | | | |
| Preview-before-write contract (`resolve-schedule`: validated cron + description + next-5 fires) | cronlite | STEAL | Roster-CRUD-with-preview, pre-built, deterministic |
| Tool annotations (destructive/read-only hints) + partial-update ignored-fields honesty | cronlite + ttm | STEAL | Discipline that makes a small tool surface trustworthy |
| Handshake-identity self-reference guard | mcp-cron | STEAL | They hit recursive self-scheduling live; any MCP-exposing scheduler needs it |
| Refuse-to-bind HTTP without a token | cronicle | STEAL | For the streamable-HTTP transport when it lands |
| `install_daemon` as an MCP tool | claude-cron | REJECT | Privilege escalation as a feature; installation is a human CLI verb |
| Ungated free-text task creation | 5 of 8 projects | REJECT | The Hermes #8886 hole, independently rebuilt five times |
| **Security / config** | | | |
| Declared typed invocation block as the roster-entry template | obelisk `[[activity_exec]]` | STEAL | The only prior art with pre-declared, no-free-text jobs; our roster schema starts here |
| Validate at declaration time, not fire time | cronlite (SSRF placement) + ttm (anti-lesson) | STEAL placement | ttm's charset-only validation = silently-dead jobs; admission runs the real parser |
| Definition (file) vs control-state (DB rows) separation | cronicle | STEAL | Resolves our write-back tension: pause/auto-pause are ledger rows; roster edits are the only write-back |
| Parse failure keeps prior roster | cronicle | STEAL | Reload must never brick the schedule |
| Diff-and-reschedule reconcile core | claude-tasks `SyncTasks` | ADAPT | Keyed-map diff maps cleanly onto the TOML minute-loop reload, minus the DB |
| Sub-second / inotify config reload | cronicle (1s) / geta (200ms) | REJECT | Wakeup churn for minute-granularity cron; reload at tick |
| Startup cross-field sanity warnings | cronlite | STEAL | Fold into orario boot and `config lint` |
| DB as source of truth for jobs | 5 of 8 projects | REJECT | Nothing diffable, reviewable, or git-tracked; the custode pattern exists for this |
| **Deployment** | | | |
| Service-install verb (generate plist/unit, enable, print fallback) | task-trigger-mcp | STEAL | Small, complete, matches supervised-BY-launchd exactly |
| Oneshot runner under OS timer (no daemon) | claude-cron | REJECT v1 (!) | Report graded STEAL-consider-default; overruled -- see ruling 5 |
| `caffeinate -i -w $PID` prevent-sleep | mcp-cron | ADAPT (v2) (!) | Report graded STEAL; deferred -- see ruling 6 |
| Ignore-SIGTERM orphan daemon | mcp-cron | REJECT | Unsupervised, invisible, unrestartable; the inverse of our stance |
| Derived dead-man thresholds (window = worst-case legit runtime + margin) | cronlite reconciler | STEAL | Compute the window from the verb's own timeout + retry policy, never hand-guess |
| Distributed plane (leader election, queues, worker registries, Postgres) | cronlite / cronicle | REJECT | N=1, supervised; competent code solving a problem we refuse to have |

### 6.2 The v1 cut

Ordered by build dependency. v1 = one laptop, ~5 routines, useful.

1. **Roster TOML schema + parser** -- obelisk-shaped entries (verb name + pinned args, cron, tz, timeout, `if_busy`, `catch_up`, `expires`, env allowlist, optional manual-only rationale). Real-parser validation at admission; startup sanity warnings; `roba config lint` integration.
2. **SQLite ledger** -- WAL + busy_timeout, `UNIQUE (entry, scheduled_at)`, terminal-state guard in SQL, claudecron-shaped run row + receipt path/digest, control-state tables (pause rows, cronicle-style).
3. **Minute loop** -- tz-aware `next_fire` computed and persisted; CAS claim; `catch_up none|once` via cronlite's window logic capped at 1 with skipped-occurrence rows; reload-at-tick with SyncTasks-style diff and keep-prior-on-parse-failure; tick-drift recorded.
4. **Executor** -- spawn the pre-declared roba verb, process group + WaitDelay escalation, wall-clock timeout, env allowlist + `ROBA_RUN_*` context env, stderr to orario's log, `${last_run}` substitution resolved from the ledger.
5. **Overlap** -- `if_busy skip` with missed-fire receipts (queue deferred).
6. **Outcome classifier + policy table** -- typed exits 0-7: 2 = halt-all + alert; 4 = bounded retry; 5/7 = note; `skipped` a distinct status, never `failed`.
7. **Receipt join + dead-man check** -- expected receipt absent past a derived window (verb timeout + retry + margin) = alert.
8. **Daily budget rail + auto-pause** -- summed observed `cost_usd` from receipts; pause is a control-state row, not a roster edit.
9. **ntfy.sh notifier** -- for halt-all, dead-man, auto-pause. One POST.
10. **MCP plane (stdio)** -- roster CRUD with resolve-schedule preview and gated TOML write-back; `run_now`; read tools (runs, spend, next_fires); the SQL query tool; tool annotations; self-reference guard.
11. **`install-service` verb** -- plist/unit generation, supervised by launchd/systemd.

**v2+ deferrals:** `if_busy queue`; OAuth usage-window gate (first in line -- fail-closed, snooze-to-reset); streamable-HTTP transport with refuse-to-bind auth; per-verb circuit breaker; Discord/Slack webhook payloads; prevent-sleep opt-in; interval sugar (`every = "30m"`); file-watch (maybe-never).

**Never:** LLM anywhere in the daemon; free-text prompts or shell strings in the roster; generic retry engine; DAG/dependencies; cluster/leadership; dashboard; sub-minute; secrets store; Prometheus; embedded agent loop; webhook execution model; MCP-driven daemon installation.

### 6.3 Design rulings

1. **Spawn-and-wait, not detach+receipts, as the execution mechanism.** Orario spawns roba synchronously in an owned process group and waits; the receipt is the crash-safe *record*, not the execution vehicle. Evidence: ttm's pid-less orphaning and claude-tasks' no-pgroup kill show that anything the scheduler does not hold in a process group becomes immortal and invisible. Detach would recreate exactly that. Receipts still close the crash window: on restart, reconcile in-flight ledger rows against receipts (the stuck-`running` wedge appears in claude-cron, claudecron, and claude-tasks).
2. **TOML file = definition truth; SQLite = runtime/control truth.** Cronicle's separation ("HCL says what to run; schedule_state says is it allowed to run") is the resolved shape. Five projects with DB-as-truth all ended up with unreviewable, undiffable job stores. Auto-pause and manual pause are ledger rows; the gated write-back path is reserved for genuine roster edits.
3. **NL schedules enter at authoring time only, compiled to cron with preview.** Cronlite's `resolve-schedule` contract (validated expression + description + next-5 fires, sibling tools steered to call it first) plus claude-cron's parser-precedence lesson (chrono-node silently eats "every monday at 8" as a one-time date). Claudecron's in-daemon variant is the counterexample: LLM in the loop, broken cache semantics.
4. **SQL query observability tool: yes.** mcp-cron's mechanism is complete and ~70 lines: SELECT/WITH prefix gate as UX, `PRAGMA query_only=ON` on a dedicated connection as enforcement, hard row cap with truncation warning, live DDL embedded in the tool description. Strictly better than hand-designing N bespoke query tools.
5. **Persistent minute-loop daemon, not oneshot-under-OS-timer.** The claude-cron read argued the oneshot shape should be the default; overruled. A roba run can take 30+ minutes: under launchd label single-instancing, a long run blocks every subsequent scan (claude-cron documents this delay itself), and `if_busy queue`, dead-man timing, and the MCP plane all want residency. Keep the oneshot insight as the *supervision* posture: launchd KeepAlive owns the process; a crash costs one tick.
6. **Prevent-sleep: not v1.** `catch_up = once` plus tick-drift detection is the laptop-honest answer to sleep; mcp-cron's `caffeinate -i -w $PID` (self-reaping, 12 lines) is banked as a v2 opt-in per-entry flag for must-run-tonight work.
7. **Triggers beyond cron in v1: manual fire only.** Manual is a ledger poke through the identical path (cronicle's unification invariant), so it costs nothing. File-watch, hooks, and dependencies each failed in prior art in ways that confirm the refusal (broken hook delivery, volatile dependency state, resident watchers).
8. **Budget is observed, never computed.** Cronicle and claude-cron both maintain price tables or fabricate exchange rates, and both drift or lie. Orario sums `cost_usd` from roba's receipts -- the runner that spent the money reports the spend.

### 6.4 What nobody built

The honest novel-work list -- zero prior implementation across all 8 projects:

- **Typed exit-code -> declarative policy table.** Every project either discards the exit code or maps nonzero to generic failure. The 2=halt-all / 4=bounded-retry / 5,7=note classifier is built from scratch.
- **Observed-cost daily rails + auto-pause.** No project has any spend governance wired to real cost data.
- **Gated write-back to a reviewed config file** (custode pattern). No project has a config file for jobs at all, let alone MCP-mediated, previewed, diffable edits to one.
- **Dead-man detection on absent receipts.** Cronlite's reconciler is the nearest neighbor but watches orphaned rows it created; "expected receipt never appeared" is a different and unbuilt check.
- **`if_busy` as explicit per-entry policy.** Skip approximations exist (ttm); `queue` exists nowhere.
- **The receipt-join architecture itself** -- a scheduler whose ledger joins an external runner's durable run record instead of owning output. Every project either absorbs output into its DB or (cronlite) is blind to outcomes entirely.
- **Deliberate per-entry `catch_up none|once` with skipped-occurrence records.** Only accidental variants exist.

This is roughly the top half of the v1 list -- the build estimate should treat items 5-8 of the cut as green-field, with items 1-4 and 9-11 assembled largely from the steals above.

### 6.5 Deprecation verdict

**task-trigger-mcp: DEPRECATED -- confirmed, with a twist.** Evidence: `Cargo.toml` description on crates.io v1.4.0 (published 2026-04-10) reads "DEPRECATED: use agent-canopy instead"; `main.rs` at HEAD is a 6-line stub that prints the deprecation and exits 1, so the published binary cannot run; the deprecation commit and rename issue (#36) are on record; last functional release was v1.3.0 (2026-03-28); lifetime downloads 108. The twist: the named successor crate `agent-canopy` does not exist on crates.io -- the real successor is the GitHub-only `UniverLab/harness-canopy`, an "agent operations center" that scope-crept out of the sharp-scheduler niche entirely. The only Rust occupant of this niche abandoned it for a platform. The niche is empty, which is the strongest external validation that a sharp, refusal-driven, laptop-first scheduler is unbuilt territory.

## 7. Open questions (updated post-deep-dive)

1. ~~Spawn-and-wait vs `--detach` + receipts~~ **RESOLVED by ruling 6.3.1:
   spawn-and-wait in an owned process group; the receipt is the crash-safe
   record, not the execution vehicle.** Prior art shows anything not held in
   a process group becomes immortal and invisible.
2. ~~Where does catch-up live?~~ **RESOLVED: per-entry `catch_up none|once`
   with skipped-occurrence records; tick-drift measurement doubles as sleep
   detection. Prevent-sleep (`caffeinate`) deferred to a v2 per-entry flag.**
3. **The custode question.** Parallel realizations (Elixir fleet + Rust
   spine) or convergence (custode eventually firing through this)? No need
   to decide before the spine exists and is dogfooded.
4. **Market-size signal.** Three dead prior attempts -- including the only
   Rust occupant abandoning the niche for a platform -- says the niche has
   no audience. roba's answer (build for ourselves, publish anyway) probably
   applies, but it caps how much polish the tool deserves.
5. **The name.** orario fits; verify no collision with the 2026-06-17 test
   project.
6. **How much of #390 survives?** The protocol layer (labels, status-comment
   grammar, reaper semantics) is scheduler-agnostic and stays; only its
   launchd-plists-and-shell-glue firing layer gets superseded.
7. **NEW: roba-side prerequisites.** Promote the receipt schema
   (`src/receipt.rs`) into roba-types (the scheduler is the second published-
   contract consumer). Possibly also: a receipt carries `cost_usd` today via
   the envelope -- verify the scheduler can sum spend from receipts alone, or
   file the gap on roba.
8. **NEW: `${last_run}` token contract.** The highest-value unspecced steal
   (cronicle): "since last run" verbs need orario to pass the last successful
   fire time into the roba invocation -- decide flag vs env vs template var.
