# Inspectable context planning

> Status: typed planning, explicit host inputs, and MCP read evidence
> implemented; bootstrap, acknowledgement, gating, and isolation controls
> remain incremental work tracked by GitHub issue #489.

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
- operator, provider, or shared audience;
- provider-baseline through turn-level declared precedence;
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

## Host-supplied MCP context

`ContextPlan::builder_from_run_spec` begins with that exact executable
inventory. A host may then add inline or externally available entries with
explicit audience, precedence, origin, lifecycle, delivery, freshness, and
sensitivity metadata. `AgentInstance::new_with_context_plan` accepts the
resulting immutable plan only when every provider-adapter entry still matches
the suspended `RunSpec` metadata and material. Missing or replaced executable
context therefore fails before admission, provider launch, or private endpoint
binding.

The operator projection is the administrative superset and can inspect the
complete plan. The provider projection contains only entries whose audience is
`provider` or `both`; an operator-only ID resolves as absent over the private
provider endpoint. Entries are stably ordered from lower to higher declared
precedence while retaining insertion order inside one layer.

This precedence is Roba's declared composition order, not a claim about hidden
provider policy. Managed or provider-native ambient instructions may have
their own precedence that Roba cannot override or fully observe. The current
slice records ordering and makes it inspectable; automatic replacement,
conflict inference, and linting remain separate work.

## MCP context contract

Both projections publish two resource capabilities:

- `roba://context` returns `ContextSnapshot`, containing the content-free
  Roba-declared manifest and current or latest provider read evidence;
- `roba://context/entry{?id,generation}` returns one explicitly requested
  `ContextContent` value.

The provider projection additionally publishes equivalent read-only
`context.manifest` and `context.read` tools. Claude Code can expose MCP
resources to the model, while the provider-neutral portable contract cannot
assume every adapter does so. The tools are therefore included in the exact
native provider allowlist and preserve the same generation fencing, typed
structured content, redaction, and evidence semantics as the resources.

The control projection can inspect the fixed plan before any turn. During a
turn it reports that active operation's evidence; after settlement it retains
the latest operation's final evidence. Control-side reads do not masquerade as
provider acquisition.

The provider projection is bound to one exact operation. Its manifest and
manifest fingerprint cover only provider-visible entries. Its manifest read and
every successful entry read, through either MCP form, record the operation id,
context generation, manifest or entry fingerprint, first and last observed
timestamps, and a saturating count. The private endpoint stops accepting and
drains requests before operation settlement, so retained evidence cannot miss
a late successful read. A stale generation, unknown entry, expired operation,
or unavailable body fails closed.

The generation identifies a content revision, not a turn counter. The fixed
plan currently stays at generation 1 across repeated operations; the separate
operation id prevents evidence from one run satisfying a later run. Future
live context changes will advance the generation without redefining operation
identity.

Public and redacted entries may be returned by the explicitly content-bearing
entry resource. Their bodies remain absent from manifests, snapshots, and
debug formatting. Secret entries receive no body-derived fingerprint and are
unavailable through the generic content resource. Existing `RunSpec` entries
retain `provider_adapter` as their primary delivery because the adapters still
inject them every turn; MCP currently provides an inspectable second path, not
a silent change in provider prompt behavior.

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

The contract distinguishes these states instead of collapsing them:

1. **planned**: the host selected an entry;
2. **available**: the entry exists in the provider's MCP projection;
3. **delivered/read**: the provider requested it; this state is implemented;
4. **acknowledged**: the provider explicitly reported acquisition;
5. **followed**: an inference from behavior, never a mechanical guarantee.

For unattended mutation, the intended forcing function is generation-scoped
capability gating: required context must be read or acknowledged before the
provider projection exposes mutating tools. Authority remains a separate typed
contract; prose cannot grant it.

## Next slices

1. Compile the minimal launch bootstrap from required manifest entries.
2. Stop reinjecting unchanged stable context into resumed sessions where the
   provider mechanics prove that omission is safe.
3. Add context linting for duplicate fingerprints, precedence conflicts,
   conflicting instructions, unsafe locators, and excessive prompt weight.
4. Mechanically inventory clean-home, ambient, controlled, fresh, resume, and
   steering behavior for both built-in providers.
5. Add explicit acknowledgement and gate provider-facing mutation on the
   required evidence policy.
6. Apply the same manifest to parent-spawned Robas without inheriting the
   parent's transcript or ambient environment by accident.

## Sources

- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-usage)
- [Claude Code settings and precedence](https://code.claude.com/docs/en/settings)
- [Claude Code context diagnostics](https://code.claude.com/docs/en/debug-your-config)
- [Claude Code memory](https://code.claude.com/docs/en/memory)
- [Claude Code MCP](https://code.claude.com/docs/en/mcp)
- [Codex configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
- [Codex `AGENTS.md` discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md)
- [Codex MCP](https://learn.chatgpt.com/docs/extend/mcp)
- [Codex non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode)

## Current non-goals

- no universal system prompt;
- no claim that MCP availability means the provider requested context;
- no serialization of prompt bodies, secrets, or provider-private sessions;
- no provider-independent claim of hermetic behavior;
- no workflow, issue, repository, or parent-child policy in `roba-core`;
- no claim that an MCP read means the model acknowledged or followed content;
- no mutation gating or context update API yet.
