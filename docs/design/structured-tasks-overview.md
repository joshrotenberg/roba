> Status: PARKED sketch (2026-06 era). The structured-tasks direction
> was overtaken by the verbs/profiles arc (committed roba.toml verbs,
> the persona-as-profile shape) and never built as designed.

# Structured Agent Tasks — Direction & v1 Spec

> Orientation for a working session. This is the *where we're going*, not a
> finished design. Read it, then help build the v1 surface described at the end.

## Thesis

For common, repetitive repo tasks, the prompt and the response are **predictable
in shape**. The natural-language framing around them is mostly ceremony. So:

1. Make the **input** as structured and deterministic as possible.
2. Make the **output** as structured and deterministic as possible.
3. Treat the agent session as a **gray-box substrate** — opaque reasoning in the
   middle, but *bookended* by enforced setup and enforced output. We don't
   micromanage how it thinks; we constrain what it's allowed to start with and
   what it's allowed to return.

The real move is **relocating the validator**. Conversational AI ships fluent
nothing because the only thing checking the output is a human who feels like an
insight happened. Replace that check with one that either parses or doesn't, and
the whole register of load-bearing-nothing has nowhere to live.

## Design principles

| Principle | Meaning |
|---|---|
| Schema is the protocol | The surface schema is the spec. CLI/MCP/layer is just an encoding. |
| No natural language across the boundary | No free-text field in a response. Explanations are typed `enum` + structured context. IDs/refs/paths/shas are fine — they're references, not prose. |
| Typed failure is mandatory | Every way a task can stop that isn't success is an enum variant, not a sentence. If you want to write a paragraph, you're missing a variant. |
| Schema-valid ≠ true | `"branch clean"` passes a schema and can still be a lie. Truth is checked by re-deriving from the repo, not by trusting the model. |
| Don't over-schematize | Schema the parts that are genuinely typed (verdict, action, params). An irreducibly fuzzy part gets **one** clearly-marked free-text field that pays an explicit tax — and nothing load-bearing lives only there. |
| Gray box, not white box | Constrain the bookends; leave the reasoning opaque. We're not trying to control the model's chain of thought, only its inputs and outputs. |

## Build on roba — don't rebuild it

roba already is the substrate. It sits on `claude-wrapper`, so wrapping the
wrapper ourselves would just duplicate it. What it already gives us:

| Capability | roba mechanism |
|---|---|
| Task as a verb | Aliases — `roba fix 123` expands a template + flag bundle |
| Request params | Alias `args` + `${...}` substitution |
| **Output schema enforcement** | `--json-schema PATH` — claude validates; result surfaces under `.result.*` |
| Enforced setup | `[profile.*]`: `readonly`, `append_system_prompt`, `max_turns`, `max_budget_usd` |
| Async / monitor | `--detach` returns an id; `roba show <id> --wait` polls. Handle = session id. |
| Scripting ABI | Typed exit codes, versioned `--json` envelope, `refusal` field |

The "deterministic bookends" reviewer example is the pattern we're generalizing:
profile pins setup *and* output contract, only the reasoning is left to the model.

## v1 surface

Three aliases, each pinning a profile and its own response schema. The scope set
*is* the alias namespace — no scope enum/flag needed.

| Alias | Profile | Mutates repo? | Response schema |
|---|---|---|---|
| `roba triage <issue>` | readonly, low cap | no | verdict + typed reason |
| `roba fix <issue>` | writable, higher cap | yes | branch / commits / pr / files |
| `roba reproduce <issue>` | readonly | no | `reproducible: bool` + context |

### Starter enums

These are intentionally small. We will find more variants — that's expected and
correct. Adding a variant is the supported way to grow; reaching for a string is not.

- **Scope (the three aliases):** `triage`, `fix`, `reproduce`
- **Outcome (a result field, not an exit code):** `done`, `partial`, `blocked`, `rejected`
- **Blocker reason:** `dirty_tree`, `ambiguous_ref`, `ci_failing`, `not_reproducible`, `needs_human`
  - `needs_human` is the deliberate catch-all of last resort.

### Schema source of truth

Response schemas are **generated from Rust types** (serde + schemars), so the
wire contract and the typed model can't drift. The JSON Schema files handed to
`--json-schema` are build artifacts, not hand-maintained.

## The two things roba does *not* give us — these are the actual work

1. **Domain outcome ≠ process exit code.** roba exits `0` for any successful
   *run*, even if the issue wasn't fixed. So `done/partial/blocked/rejected`
   lives **inside** the schema'd result; consumers branch on `.result.outcome`,
   not `$?`. A shell-native exit code requires a thin wrapper that re-maps
   `.result.outcome → exit code` — build that only if a pipeline needs it.

2. **Reality check.** The only genuinely new code. A post-step re-derives the
   claimed result from `git`/`gh` and compares:
   `roba fix 123 --json | reality-check`. Shell script for v1; promote to a
   crate only if it earns it.

## Reserved: typed report channel (push, not poll)

**Not built in v1. Reserved so it slots in without reshaping anything.**

The idea: roba hosts an **in-process MCP server** and injects an `--mcp-config`
pointing the spawned claude back at it. roba already configures MCP servers *for*
the agent (tools it calls to do work); this is the same wiring reversed — a tool
the agent calls to **report** typed intermediate/final status (push), instead of
roba scraping `--trace` or polling `show --wait` (poll). One stack (tower-mcp)
hosts both directions.

What it would buy:

| Gain | vs poll today |
|---|---|
| Typed intermediate status | Progress stops being unstructured stderr; becomes enum + struct |
| In-loop validation | `--json-schema` rejects bad output post-hoc (whole run fails); a server rejects a bad `report(...)` *at call time* — model can correct mid-run |

**v1 posture: tools present, agent may ignore them.** We ship the report tools
wired up but make nothing depend on them. Consequence, stated honestly: an
optional tool is *advisory* — same "asking nicely" gap the reviewer example
warns about (permissions block, skills ask). So in v1 the channel is
**supplementary telemetry only**; stdout + `--json-schema` remains the sole
authoritative owner of the final outcome. That keeps the regression contained:
if the agent skips reporting, we lose checkpoints, not correctness.

To make it authoritative later (deferred), add a **forcing function**: the final
schema requires a `report_token` issued only by a successful `report_final(...)`
call, so valid output is unmintable without speaking the typed channel.

Constraints to honor whenever this is built:

- **One authoritative owner per field.** Don't let `outcome` arrive via both a
  mid-run `report_final` and stdout. Either the channel is intermediate-only
  (stdout owns final), or `report_final` *is* the output and stdout mirrors it.
  A disagreement between the two is the failure mode to design out.
- **Doesn't fix schema-valid ≠ true.** A typed report is still a *claim*; the
  server checks it against the schema, never the repo. Reality-check is unchanged
  — and this adds more claim surfaces (every report), so only hard-check the final.

Design the progress/report type into the schema now (even unused) so the channel
is a drop-in later.

## Open decisions

| # | Decision | Current lean |
|---|---|---|
| 1 | Enum-validate scope, or let the three aliases be the enum? | Aliases — roba-native |
| 2 | Outcome→exit-code wrapper, or parse `.outcome`? | Parse for now |
| 3 | `blocker.reason` in our schema, or roba's generic `refusal`? | Our schema |
| 4 | Custom `monitor` block, or just `--detach`/`show --wait`? | Drop it; session id is the handle |
| 5 | `reality-check` shell or crate for v1? | Shell |
| 6 | Report channel: push (MCP) or poll (`detach`/`show`)? | Both wired, poll authoritative; push is optional telemetry in v1 |

## Non-goals (v1)

- No new daemon, orchestrator, or platform. roba's scope discipline holds.
- No multi-turn conversation. Single-shot request → structured response.
- No MCP server yet for *external consumers*. The reserved report channel is an
  *internal* roba→agent loop, present but advisory in v1 — no forcing function.
- No attempt to constrain the model's *reasoning* — only its inputs and outputs.

## First concrete step

Draft the `fix` path end-to-end as the template the other two copy:

1. `[profile.fix-work]` block (writable, caps, `append_system_prompt`, `json_schema`).
2. `[alias.fix]` block (`args`, `flags`, `template` pulling issue context via `gh`).
3. `fix.schema.json` generated from the Rust result type (outcome + result + blocker).
4. A ~30-line `reality-check` that validates a `fix` response against the repo.

Then `triage` and `reproduce` are copies with narrower schemas and readonly profiles.
