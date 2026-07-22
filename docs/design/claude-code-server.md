> Status: PARKED design concept (2026-07-22). The server-as-adapter
> direction was ratified closed in #442; the code was removed in #450.
> Kept as the design record; the reopen trigger is a real multi-turn
> duplex need that a one-shot run cannot serve.

# The Claude Code Server

A concept for driving a single, warm Claude Code session programmatically.

## What it is

A Claude Code server is a long-lived process that holds **one** Claude Code
session open and exposes it behind a standard programmatic interface. Instead of
a human typing into an interactive session, other programs send it prompts turn
by turn and read the responses. The session stays **warm** between turns: its
context, cost, and history accumulate across the whole life of the process.

It is not a platform, a scheduler, or a multi-tenant service. It is the thinnest
useful thing that turns "an interactive coding agent" into "an addressable,
scriptable, observable one" without giving up session continuity.

## The gap it fills

There are two normal ways to run a coding agent, and each gives something up:

- **Interactive** (a terminal UI you type into). Stateful and capable, but you
  have to be *in* it: no composition with other tools, output you cannot
  capture, and you leave your shell to use it.
- **One-shot headless** (`claude -p "..."`). Scriptable and composable, but
  **cold** every time: no context across calls, no session continuity, and no
  view into what it did beyond its final output.

The server keeps the continuity of the interactive session and the composability
of the headless one at once: a warm session you talk to from code.

## The core model: one process = one session = one endpoint

The defining choice is what the server *refuses* to be. It is **not** a session
pool. There is no session id in the protocol, no routing table, no eviction
policy, no lifecycle API for creating and destroying sessions.

**The process is the session.** One process holds exactly one warm session and
exposes exactly one endpoint. Want ten sessions? Run ten processes. The operating
system is the pool.

This is the load-bearing simplification. A general "session server" has to own a
routing layer (which request goes to which session), a ledger (which sessions
exist and what state they hold), an eviction policy (when to reap idle sessions),
and a lifecycle API. That machinery is where such designs get complex and
fragile. Collapsing to one session per process deletes the entire layer: the
server becomes a thin broker over a single child, and durable state lives in the
agent's own on-disk session records rather than in the server.

## The interface

Expose the session over a standard agent-tool protocol, so any client speaks to
it the same way it speaks to any other tool. The Model Context Protocol (MCP) is
the natural fit: the session becomes a tool the caller invokes with a prompt and
reads a response from, plus a status call for live figures (cost, turns, session
id).

A single serializing actor sits in front of the session: one mailbox, processed
first-in-first-out. This makes "one turn at a time" a structural guarantee rather
than a convention. Concurrent callers queue; they do not interleave inside the
agent's reasoning loop.

Because the session is warm, each turn builds on the last, and the server tracks
cumulative cost and turn count. An optional spend ceiling hard-stops the session
once it is exceeded.

## What the model buys you

- **Warm.** Context persists across turns. No cold re-spawn, no re-paying the
  session's warm-up on every call.
- **Addressable.** A stable endpoint you send prompts to, from any program.
- **Observable.** The server can narrate the session's activity -- the tool calls
  it makes -- as it happens, so a driving program can see what the session is
  doing without scraping logs.
- **Bounded.** Budget and turn caps give the session a known, capped blast
  radius.
- **Role-configured.** The session can be launched as a specific role: a named
  agent with a fixed instruction and capability surface, so "what this session
  is" is set once, for its life.
- **Surface-bounded.** You can constrain exactly what the session can see and do.
  A known capability and instruction surface is what makes an autonomous,
  unattended session trustworthy.

## Two directions of communication

The obvious direction is inbound: clients drive the session (send a prompt, get a
response, ask for status). Two less obvious directions make the pattern more than
a remote procedure call:

- **Reflexive (the session inspecting itself).** The server can hand the running
  session a tool that describes its own execution -- what role it is running as,
  its remaining budget, how many turns it has taken. The agent can reason about
  its own context, not only the task.
- **Operator bridge (the session asking a human).** The server can route a
  question from the running agent to a human operator and carry the answer back
  mid-turn. This gives bidirectional human-to-agent communication that neither a
  one-shot call nor a plain tool protocol provides: the agent can pause to ask
  for a decision or a clarification instead of guessing.

## Structured or prose I/O

A session can be pinned to an output schema at launch, so every turn returns a
validated structured object instead of prose. This is a per-*session* mode, not a
per-call option: the schema is fixed when the process starts. A different schema
is a different process, which fits the one-process-one-session model exactly.

## How it differs from the alternatives

- **vs an interactive session:** same continuity, but headless, scriptable, and
  observable. You drive it from code instead of living in it.
- **vs one-shot headless:** same composability, but warm (context across turns),
  addressable, and observable.
- **vs a hosted / managed agent service:** those run the agent loop for you on
  their infrastructure, at the API layer, billed and hosted. A Claude Code server
  is the **local** counterpart: it drives the agent binary on your machine, with
  your files and your existing auth. Same "drive an agent programmatically" goal,
  a different layer.
- **vs a session-pool server:** deliberately not a pool. Multiplicity is N
  processes, not N sessions in one process, trading a little per-process overhead
  for the deletion of the routing, ledger, and eviction layers.

## Scaling: multiplicity without a pool

If you want many sessions, run many processes. If you also want to address them
by name rather than by process, add a thin discovery layer *over* the
one-per-process model: a map from a name to a process handle (a pid or a port),
which is a reconstructible cache, not owned state. You get name-addressing and
crash/resource isolation without a fat multi-session server. The lean default
stays one session per process; the discovery layer is optional and external.

## Boundaries and open questions

- **One vs N sessions per process.** The discovery-layer approach above gets most
  of the ergonomics of a multi-session server without its complexity. A true
  multi-session server is only warranted if thin clients must create sessions by
  name against a shared server (server-side name resolution), rather than a
  config-aware client resolving names itself.
- **Hermeticity.** Fully bounding a session's surface depends on the underlying
  agent being able to receive its entire context explicitly and inherit nothing
  ambient. How completely that is achievable is a property of the agent, not the
  server.
- **Persistence.** Sessions are ephemeral to the process. Work that must outlive
  a run has to ride on durable external state -- a commit on disk, a pull
  request, the agent's own session record -- never on process memory. A crash
  loses the live session but not the work already committed.

## When to reach for it

Reach for a Claude Code server when you want programmatic, multi-turn work against
a *local* agent: an addressable worker, an observable and cost-bounded session, a
reflexive self-aware run, or a human-in-the-loop autonomous task.

Do not reach for it as a durable job queue (use an external queue that survives
restarts), as a hosted multi-tenant service (that is the managed-agents layer),
or for a single stateless question (a one-shot call is simpler).
