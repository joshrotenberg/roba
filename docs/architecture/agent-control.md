# Agent control semantics

Roba exposes one hot logical agent with at most one active finite operation.
This document defines the distinction between turn admission, follow-up work,
configuration, interruption, and shutdown.

Provider continuity policy and idle rotation are specified separately in
[`session-lifecycle.md`](session-lifecycle.md).

## `agent.turn`

`agent.turn` admits work only while the agent is idle. A second call while an
operation is active receives a typed `busy` refusal; Roba does not hide a work
queue in the base contract.

The required `text` becomes the first provider turn. Optional `overrides` may
set `model`, `effort`, and the `max_turns`, `max_cost_usd`, or `timeout_secs`
limits for this operation. Overrides cannot change provider identity, working
directory, permission posture, tool authority, context, extensions, or
session identity. They apply to every provider turn in the admitted operation
and disappear at settlement. `roba://agent.active_configuration` and terminal
turn metadata expose the effective values.

## `agent.follow_up`

`agent.follow_up` is not live prompt injection. It addresses one exact active
operation and appends a prompt to that operation's bounded FIFO. After the
current provider turn completes, Roba resumes the provider session with the
oldest follow-up. Multiple follow-ups retain submission order. The queue holds
at most 16 prompts; further calls receive a typed `queue_full` refusal.

The originating `agent.turn` call or Task remains active across follow-ups and
settles only after the FIFO is empty or the operation fails or is cancelled.
Its terminal outcome is the last provider outcome. `turns_completed` and the
event journal retain the complete boundary history. `follow_up_queued` records
admission to the FIFO and `follow_up_applied` records consumption immediately
before the resumed provider turn starts.

The provider must support session resume. If it does not, follow-up is refused.
If a successful fresh turn supplies no usable session handle, the operation
fails when Roba attempts to apply its first queued follow-up.

## Interruption and shutdown

`agent.interrupt` cancels one exact active operation, waits for settlement,
and leaves the logical agent reusable. `agent.shutdown` closes admission,
drains active work, and permanently stops the host. Operation ids fence all
controls so a delayed client request cannot affect later work.

## Configuration lifetime

The hosted configuration is immutable for the lifetime of the current
`AgentInstance`. There is no `agent.config` mutation tool. Persisted/default
configuration and reload semantics belong to the configuration work tracked
separately; adding mutation before that contract exists would create a second,
competing source of truth. Operation-local `agent.turn.overrides` are the only
runtime configuration changes in this layer.
