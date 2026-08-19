# Provider session lifecycle

Roba separates one logical agent from the provider-owned physical sessions
used to execute its operations. The logical agent has a monotonic session
generation, a provider-neutral continuity policy, and at most one validated
opaque provider session handle. Provider session identifiers remain private
evidence; operator status exposes only availability and generation metadata.

## Policies

The startup setting `[session].mode` and the `--session-mode` override select
one policy for the lifetime of an `AgentInstance`:

| Mode | Behavior |
| --- | --- |
| `sticky` | Retain the latest validated provider session until clean rotation or shutdown. This is the default. |
| `fresh` | Start every admitted operation in a new provider session. Each admitted operation after the first advances the generation. |
| `managed` | Retain sessions under host policy. The current implementation rotates only on an explicit request. |

An explicit `--resume` seed is valid with `sticky` and `managed`. It conflicts
with `fresh` and fails during startup rather than being silently ignored.

The policy is host state, not executable provider intent. It stays outside
serialized `RunSpec`, cannot be changed by an operation-local override, and is
shared by the finite CLI and hot MCP hosts through the same startup resolver.

## Generation evidence

`roba://agent` publishes a content-free `session` snapshot:

- the effective policy;
- a generation number starting at `1`;
- whether validated provider continuity is available;
- the number of actual core provider turns observed in the generation;
- generation start and age timing when the system clock is available.

The turn count comes from authoritative core `TurnStarted` events. It includes
follow-ups and failed provider turns rather than trusting provider-reported
usage metadata. It does not expose provider session identifiers.

A generation is a Roba host boundary, not a provider claim. For `sticky` and
`managed`, it spans operations until rotation. For `fresh`, it identifies the
operation-scoped continuity epoch even though no provider handle is retained.

## Clean rotation

The operator-only `agent.session.rotate` tool accepts:

```json
{
  "expected_generation": 1,
  "strategy": "clean"
}
```

Clean rotation drops retained provider continuity, advances the generation
exactly once, resets its turn count and timing, and leaves the logical agent
idle and reusable. It performs no provider call and creates no summary.

Rotation is fail-closed:

- it is accepted only while the agent is open and idle;
- `expected_generation` fences delayed clients from rotating newer state;
- active, stopping, stopped, stale, and exhausted states return typed
  refusals;
- the next operation starts fresh and may establish new validated continuity.

The original `agent.turn` result remains the only place where a valid opaque
provider session handle may appear. Status, events, and rotation results stay
redacted.

## Current phase boundary

This first slice provides deterministic policy, safe observation, and manual
clean rotation. It does not yet implement:

- age, token, turn-count, or provider-capacity rotation triggers;
- summary or handoff-packet rollover;
- provider-native compaction controls;
- automatic background maintenance;
- session rotation events or durable restart recovery.

Those require explicit trigger ordering, failure policy, evidence, and
provider capability mapping. Until then, `managed` is deliberately equivalent
to sticky retention plus the explicit generation-fenced control.
