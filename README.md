# roba

[![Crates.io](https://img.shields.io/crates/v/roba.svg)](https://crates.io/crates/roba)
[![Documentation](https://docs.rs/roba/badge.svg)](https://docs.rs/roba)
[![CI](https://github.com/joshrotenberg/roba/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/roba/actions/workflows/ci.yml)
[![Downloads](https://img.shields.io/crates/d/roba.svg)](https://crates.io/crates/roba)
[![License](https://img.shields.io/crates/l/roba.svg)](#license)

Roba is a library-first, MCP-native harness for one logical coding agent.
`roba-core` executes finite provider-neutral runs; `roba-mcp` keeps one agent
hot between those runs and exposes it through a typed MCP contract. Claude Code
and Codex are built-in provider adapters.

Roba is not a workflow engine, hidden daemon, or persistent session pool. One
process owns one agent. Higher-level clients may compose several Roba processes
without adding multi-agent routing to the base harness.

The original single-prompt Claude CLI remains supported as a compatibility
surface. Its profiles, aliases, sealed bundles, scripting ABI, and detached-run
guidance live in the [legacy CLI guide](docs/legacy-cli.md).

## Install

| Source | Command |
| --- | --- |
| crates.io | `cargo install roba` |
| Homebrew | `brew install joshrotenberg/brew/roba` |
| Prebuilt binary | [Latest release](https://github.com/joshrotenberg/roba/releases/latest) for macOS, Linux, or Windows |

Install and authenticate the provider binaries you intend to use. The legacy
command and `--provider claude` require `claude`; `--provider codex` requires
`codex`.

## Choose an interface

| Need | Interface |
| --- | --- |
| One provider-neutral finite result | `roba run` |
| One hot agent addressable over MCP | `roba serve` |
| A Rust-owned finite lifecycle | `roba-core` |
| A Rust-owned hot MCP agent | `roba-mcp` |
| Existing Claude-only profiles and scripting ABI | bare `roba PROMPT` |

The full command reference is generated from the binary. Start with
`roba --help`, `roba run --help`, or `roba serve --help`.

## Provider-neutral runs

`roba run` creates one logical agent, calls its process-local `agent.turn`
contract, and waits for the finite core run to settle:

```bash
# One blocking Codex run. Stdout is the final answer.
roba run --provider codex "inspect this repository and propose the next task"

# One editable Claude run with a provider-enforced turn limit.
roba run --provider claude --writable --max-turns 20 \
  "implement the smallest coherent fix and verify it"

# Resume a provider-owned conversation by its opaque identifier.
roba run --provider codex --resume THREAD_ID "continue from the prior result"

# Emit the terminal RunSnapshot in the versioned JSON envelope.
roba run --provider claude --json "summarize this project"

# Add the typed, repository-scoped Git observation service.
roba run --provider codex --git "inspect the current Git workspace"
```

Run flags explicitly select the provider, model, effort, instructions,
context, permissions, limits, timeout, resume identity, and optional services.
The legacy `roba.toml` profile and alias pool does not apply to this path.

## Hot MCP agents

`roba serve` starts one promptless `AgentInstance` and reserves stdin and
stdout for MCP wire data from the first byte. It accepts the same fixed agent
template flags as `roba run`, without a prompt or `--json`:

```bash
# Interactive final-protocol client with MCP Tasks.
mcp-repl --protocol final -- roba serve --provider codex

# A writable Claude agent.
mcp-repl --protocol final -- roba serve --provider claude --writable

# Keep a repository-scoped Git service available across turns.
mcp-repl --protocol final -- roba -C /path/to/repo serve --provider codex --git
```

During stable `initialize` and final `server/discover`, Roba publishes a short
operator guide explaining the single-flight lifecycle and pointing clients to
the state, context, and event resources. Clients decide how to render it;
`mcp-repl` includes it in the connection banner.

Inside `mcp-repl`, call `agent.turn text="..."`. Append `&` to create a
Task, then use `jobs`, `read roba://agent`, `read roba://events`, `wait`, or
`cancel`. `read roba://context` shows the declared context manifest and the
current or latest provider read evidence.

The base control contract is:

- `agent.turn` admits one finite operation;
- `agent.steer` queues guidance for one exact active operation;
- `agent.interrupt` cancels one operation, drains it, and keeps the agent hot;
- `agent.shutdown` permanently closes admission and drains active work;
- `roba://agent` reports redacted configuration and current state;
- `roba://events{?after,limit}` pages bounded agent-wide history;
- `roba://context` inventories declared context without its bodies, while
  `roba://context/entry{?id,generation}` performs an explicit content read.

The logical agent and validated provider session stay hot. Provider processes
do not: each accepted turn launches and settles one finite provider run. Limits
and timeout flags are per turn, not aggregate server budgets or idle deadlines.
Provider failures are typed MCP tool results and do not terminate the server.

For piped stdio, Roba consumes SIGINT so `mcp-repl` can use Ctrl-C locally. Use
`agent.shutdown`, EOF, or SIGTERM to end the server. When Roba directly owns a
terminal, Ctrl-C requests graceful shutdown.

Every admitted operation also receives a private authenticated loopback MCP
endpoint. Its base provider projection contains the read-only `self` tool and
operation-scoped context resources plus the equivalent read-only
`context.manifest` and `context.read` tools. The tool form is the portable
provider path; the resource form remains available to resource-native clients.
Turn admission and operator controls are structurally excluded. Successful
provider context reads through either form are retained against the exact
operation and generation; they do not prove model acknowledgement or
compliance. A small typed launch bootstrap identifies the operation, summarizes
authority, points to the manifest, and names mandatory MCP acquisitions without
copying their bodies into the provider prompt. Its fingerprint remains
inspectable in `roba://context`. The credential rotates and is revoked before
the operation settles. Extensions may add separately scoped provider
capabilities without copying the control router.

## Permissions and providers

Provider-neutral runs begin read-only. `--writable` grants the portable
workspace-write posture; `--full-auto` permits unattended provider operation
and belongs inside an external sandbox. Unsupported limits or permission
controls fail honestly before launch.

Codex preserves [`codex exec`'s Git-repository safety
check](https://learn.chatgpt.com/docs/non-interactive-mode). Run it inside a
Git repository; Roba does not silently add `--skip-git-repo-check`. Read-only
runs use Codex's read-only sandbox. Writable and full-auto runs use its
non-interactive workspace-write sandbox with approvals disabled. Roba never
maps full-auto to Codex's `danger-full-access` mode.

An admitted turn requires permission to bind an ephemeral IPv4 loopback
listener. If the host forbids that bind, admission returns a typed runtime
refusal before provider work begins.

## Optional Git workspace service

`--git` installs `roba-git` into `roba run` or `roba serve`. It captures the
repository containing the effective cwd once and never accepts a
caller-selected path.

`git.snapshot` and `roba://git/workspace` expose the same deterministic typed
state to the operator and active provider. Reads disable Git optional locks and
configured filesystem monitors and are bounded by a timeout.

With writable or full-auto control authority, `git.stage_all` stages tracked,
deleted, and untracked changes and returns a typed before/after receipt plus the
exact resulting index-tree object id. It refuses unresolved conflicts and
no-op requests. The provider projection remains read-only because staging may
execute repository-configured host processes beyond provider workspace-write
authority.

## Rust workspace

- [`roba-core`](crates/roba-core) -- provider-neutral specifications,
  registry, finite lifecycle, outcomes, failures, and events.
- [`roba-mcp`](crates/roba-mcp) -- one hot logical agent, typed MCP contract,
  role-scoped projections, Tasks, replay, and bindings.
- [`roba-git`](crates/roba-git) -- one fixed Git workspace exposed as typed,
  authority-scoped MCP fragments.
- [`roba-types`](crates/roba-types) -- dependency-light machine envelopes,
  exit-code constants, and receipt types.
- `roba` -- `run`, `serve`, and the original Claude compatibility command.

The retained core types are intentionally small: `RunSpec`, `Roba`, `Run`,
`RunHandle`, and `Provider`. Events are bounded and cursor-addressed; history
loss is reported explicitly. Providers return only telemetry they can observe,
so absent cost, duration, or usage never becomes an invented zero.

## Legacy Claude compatibility

The bare command remains a pipe-clean, re-enterable wrapper around
`claude -p`:

```bash
roba "summarize the Rust ownership model in three bullets"
cat err.log | roba "what is wrong here?"
roba --readonly --git-diff "is this safe to merge?"
```

It retains profiles, aliases, personas, input composition, sealed bundles,
session helpers, receipts, history, costs, a versioned JSON envelope, and typed
exit codes. See the [legacy CLI guide](docs/legacy-cli.md) for the complete
compatibility overview and unattended scripting contract. The annotated
[`roba-config.sample.toml`](roba-config.sample.toml) and every tracked example
configuration are parsed and semantically linted in CI.

## Documentation

- [Documentation map](docs/README.md)
- [Finite-core architecture](docs/architecture/core.md)
- [MCP harness architecture](docs/architecture/mcp-harness.md)
- [Legacy Claude CLI guide](docs/legacy-cli.md)
- [Annotated legacy configuration](roba-config.sample.toml)

## Status

Published on crates.io. The legacy CLI scripting surface is intended to remain
stable across `0.x`. The provider-neutral APIs and MCP contract may still
change between minor versions while they are hardened.

## License

MIT OR Apache-2.0
