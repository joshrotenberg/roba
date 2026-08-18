# Inspectable context planning

> Status: context-plan foundation implemented; MCP delivery and isolation
> controls remain incremental work tracked by GitHub issue #489.

## Decision

Context is a typed plan, not one concatenated prompt. The plan records what an
agent may know, where that material came from, when it is relevant, how it is
delivered or made available, and what evidence Roba can honestly retain.

The plan belongs to `roba-mcp`, above the finite execution core:

- `roba-core` continues to execute explicit `RunSpec` and `TurnRequest` intent;
- `roba-mcp` owns hot-agent lifetime, context generations, MCP availability,
  provenance, and eventual read or acknowledgement evidence;
- provider adapters compile the small amount of launch and turn material that
  must cross each provider's native boundary;
- extensions contribute role-scoped resources and tools rather than silently
  appending prose.

This keeps workflow policy out of the finite core while allowing a parent or
operator to construct anything from a minimally bootstrapped agent to a fully
prepared repository issue worker.

## Control spectrum

The target modes are explicit and capability-checked:

| Mode | Intended context |
| --- | --- |
| `ambient` | Preserve provider-native user and workspace discovery. |
| `controlled` | Inventory ambient sources and add one authoritative Roba bootstrap. |
| `hermetic` | Permit only declared context; fail when the adapter cannot guarantee isolation. |

`AmbientContextPolicy` records requested intent. It is not proof that a
provider honored it. Controlled and hermetic modes must remain unavailable
until their exact launch mechanics and clean-home behavior are tested for that
provider version.

## Foundation contract

`ContextPlan` retains context material in host memory and exposes a separate,
content-free `ContextManifest`. Each entry records:

- a stable ID;
- instruction, reference, authority, or session kind;
- origin kind, label, and optional safe locator;
- provider-baseline, provider-ambient, bootstrap, session, turn, or live phase;
- user, workspace, agent, operation, or turn scope;
- ambient, adapter, bootstrap, session, MCP resource, or MCP tool delivery;
- fresh-session, every-turn, generation, or dynamic freshness;
- required versus optional status;
- public, redacted, or secret sensitivity;
- a SHA-256 fingerprint when the sensitivity permits one.

Public and redacted material receive a fingerprint so plans can detect changes
without displaying bodies. Secret material receives no content-derived
fingerprint because hashes of low-entropy secrets can leak information. The
manifest-level fingerprint therefore describes all public metadata and safe
entry fingerprints but intentionally cannot distinguish two secret bodies.

`ContextPlan::from_run_spec` inventories the explicit context already present
in a suspended run template. Generated entries preserve vector order and name
their exact source fields:

- `agent.instructions[n]` -> `agent.instruction.N`;
- `context.project[n]` -> `project.context.N`;
- `context.run[n]` -> `run.context.N`.

These entries are marked `provider_adapter` and `every_turn`. That is a
characterization of current behavior, not the desired final freshness model.
`AgentInstance::context_plan` retains this inventory without spawning a
provider or changing `RunSpec` serialization.

## Current effective context inventory

The following table distinguishes Roba-controlled input from context that the
provider may load independently.

| Source | Claude adapter today | Codex adapter today | Roba evidence |
| --- | --- | --- | --- |
| `AgentSpec.instructions` | Joined into `--append-system-prompt`. | Prepended to the user prompt. | Exact ordered values exist in `RunSpec`; content-free manifest entries exist. |
| `ContextSpec.project` | Joined into the same appended system prompt. | Prepended after agent instructions. | Exact ordered values and provenance field are known. |
| `ContextSpec.run` | Joined into the same appended system prompt. | Prepended after project context. | Exact ordered values and provenance field are known. |
| Current prompt | Sent as the Claude print-mode prompt. | Fresh prompt uses stdin; resume currently uses argv through `codex-wrapper`. | Exact turn prompt is known, but is not yet a context-manifest entry. |
| Steering | Becomes another provider-native resumed turn. | Becomes another provider-native resumed turn. | Core events record the queued turn boundary. |
| Provider session | Claude session ID and transcript are provider-owned. | Codex thread ID and transcript are provider-owned. | Roba retains only the opaque handle and reported terminal evidence. |
| Private Roba MCP endpoint | Reattached through an owner-private temporary MCP config for every finite operation. | Reattached through command overrides and a child-only bearer environment variable. | Endpoint metadata is non-serializable; credentials are redacted. |
| Provider-native ambient context | User/project/local settings remain enabled; strict MCP and dynamic-prompt exclusion are not selected. | User and trusted-project configuration remain enabled; Roba does not request user-config or rules exclusion. | Potential sources are documented, but the exact loaded set is not yet observed by Roba. |

Every hot-agent operation creates a new finite `RunSpec` from the same fixed
template. The current adapters therefore repeat all three explicit context
vectors even when resuming the provider session. Characterization tests pin
this behavior so later generation-aware delivery changes cannot be accidental.

## Provider-native sources outside the current plan

Claude Code can load managed policy, command-line settings, local/project/user
settings, `CLAUDE.md`, rules, skills, hooks, MCP servers, plugins, and automatic
memory. Managed policy can outrank command-line input. Claude's own `/context`,
`/memory`, `/status`, and related commands can inspect parts of that state, but
Roba's non-interactive adapter does not yet normalize those diagnostics.

Codex can load built-in instructions, user and trusted-project `config.toml`
layers, global and nested `AGENTS.md`, rules, skills, plugins, MCP servers, and
other per-user state under `CODEX_HOME`. Current `codex exec` documentation
provides `--ignore-user-config` and `--ignore-rules`, but Roba has not yet
proven or exposed a complete controlled or hermetic launch profile.

Because neither provider offers Roba authoritative evidence of every hidden or
managed instruction, manifests must label ambient sources as provider-native
rather than claiming to reproduce their contents.

## Evidence states

Future MCP delivery must distinguish these states instead of collapsing them:

1. **planned**: the host selected an entry;
2. **available**: the entry exists in the provider's MCP projection;
3. **delivered/read**: the provider requested or received it;
4. **acknowledged**: the provider explicitly reported acquisition;
5. **followed**: an inference from behavior, never a mechanical guarantee.

For unattended mutation, the intended forcing function is generation-scoped
capability gating: required context must be read or acknowledged before the
provider projection exposes mutating tools. Authority remains a separate typed
contract; prose cannot grant it.

## Next slices

1. Add an MCP context manifest and content resources to both projections.
2. Add operation-scoped context generations and read evidence.
3. Compile the minimal launch bootstrap from required manifest entries.
4. Stop reinjecting unchanged stable context into resumed sessions where the
   provider mechanics prove that omission is safe.
5. Add context linting for duplicate fingerprints, explicit precedence,
   conflicting instructions, unsafe locators, and excessive prompt weight.
6. Mechanically inventory clean-home, ambient, controlled, fresh, resume, and
   steering behavior for both built-in providers.
7. Gate provider-facing mutation on required context acquisition.
8. Apply the same manifest to parent-spawned Robas without inheriting the
   parent's transcript or ambient environment by accident.

## Sources

- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-usage)
- [Claude Code settings and precedence](https://code.claude.com/docs/en/settings)
- [Claude Code context diagnostics](https://code.claude.com/docs/en/debug-your-config)
- [Claude Code memory](https://code.claude.com/docs/en/memory)
- [Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
- [Codex `AGENTS.md` discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md)
- [Codex non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode)

## Non-goals of the foundation

- no universal system prompt;
- no claim that MCP availability means the model read or followed context;
- no serialization of prompt bodies, secrets, or provider-private sessions;
- no provider-independent claim of hermetic behavior;
- no workflow, issue, repository, or parent-child policy in `roba-core`;
- no operator CLI or MCP context resource yet.
