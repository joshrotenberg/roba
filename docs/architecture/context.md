# Inspectable context planning

> Status: typed planning, explicit host inputs, a minimal launch bootstrap, and
> MCP read evidence implemented; acknowledgement, gating, deduplication, and
> isolation controls remain incremental work tracked by GitHub issue #489.

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

## Semantic hierarchy and configuration origin

Context has two independent axes. Its semantic role answers what the material
means; its configuration origin answers who selected it and at what
precedence. The two must not be collapsed into one profile hierarchy.

The intended semantic stack, from broadest to most specific, is:

1. the minimal Roba kernel contract that identifies the harness, current
   operation, authority, and acquisition protocol;
2. exactly one selected agent role describing the logical agent's continuing
   job;
3. small extension activation cards identifying relevant capabilities;
4. zero or more discoverable skills containing reusable methods;
5. one operation directive, often instantiated from an MCP prompt;
6. dynamic MCP resources containing current facts rather than instructions.

Origins remain separately inspectable: built-in, user/XDG, project, CLI,
parent, and operation inputs can each contribute artifacts at explicit
precedence. A skill coming from a project file is still semantically a skill;
an extension activation card supplied by a built-in remains extension context.

Only the minimal kernel is intrinsically required. Agent roles, activation
cards, skills, and prompts are Roba-managed catalog material. Provider-native
system policy and ambient files remain a separate, partially observable layer
until an adapter can enforce controlled or hermetic startup honestly.

## Unified agent contributions

MCP capabilities and context are two projections of one logical contribution,
not separate plugin systems. One installed contribution may supply scoped
tools, resources, resource templates, prompts, context entries, an exact
provider-tool manifest, and lifecycle observation.

The compilation boundary stays in `roba-mcp`. `roba-core` remains Tower-free
and receives only the finite executable run intent. The data-oriented
`roba-context` crate owns catalog definitions, bounded inline and Markdown
sources, validation, rendering, provenance, fingerprints, and deterministic
selection without owning live routers or agent lifecycle. The root host owns
startup loading; `roba-mcp` compiles the result through the ordinary extension
path into role-scoped prompts, resources, and generation-fenced context.

`AgentExtension` now supplies retained inline context or metadata-only
available context in addition to its existing MCP and lifecycle surfaces.
`AgentInstance` compiles every extension entry into the same immutable
`ContextPlan` as explicit run inputs. Invalid or duplicate IDs fail before
provider work or listener binding. Operator-only entries are absent from the
provider manifest, and retained bodies remain outside `RunSpec`, provider
prompts, serialized snapshots, and extension debug output.

The built-in harness should eventually pass through the same contribution
compiler as extensions, but as a reserved, non-removable base contribution.
This is an internal uniformity rule, not permission for an extension to
replace `agent.turn`, the context plane, or other base authority.

Extension activation is expected to become explicit:

- `disabled` contributes nothing;
- `discoverable` publishes capabilities and catalog metadata lazily;
- `eager` requires only a small activation card during bootstrap.

Installed or discoverable content is not evidence that the provider read it.
Full skill bodies stay lazy and use the existing generation-fenced read
evidence. The first proof is `roba-git`: it contributes a small discoverable
activation entry while live repository state remains resource-backed and is
never copied into the provider prompt.

## Control spectrum

The target modes are explicit and capability-checked:

| Mode | Intended context |
| --- | --- |
| `ambient` | Preserve provider-native user and workspace discovery. |
| `controlled` | Apply the adapter's tested reduction and publish the retained, suppressed, and unobservable source classes. |
| `hermetic` | Permit only declared context; fail when the adapter cannot guarantee isolation. |

`AmbientContextPolicy` records requested intent. At host construction the
selected provider must publish an exact capability profile for that policy.
The resulting `ContextSnapshot.ambient_context` records provider, requested and
effective policy, supported policies, and a safe source-class matrix. This is
evidence of adapter launch mechanics, not proof that a provider complied or
that hidden managed policy was absent.

Both built-in adapters support `ambient` and `controlled`. Neither advertises
`hermetic`, so such a request fails before private endpoint binding or provider
launch.

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

## Minimal provider-launch bootstrap

Every admitted operation compiles one typed `ContextBootstrap` from the fixed
agent configuration and the provider-visible manifest. It records:

- provider identity and exact operation id;
- read-only, workspace-write, or full-auto authority;
- that the current finite turn prompt is the current goal;
- the context manifest URI and portable manifest/read tool names;
- the context generation and provider-manifest fingerprint;
- exact mandatory acquisitions for required MCP resource or tool entries;
- a fingerprint over the complete content-free bootstrap contract.

The rendered instruction identifies the Roba MCP server, requires a manifest
read before substantive work, and names any mandatory live acquisitions. It
does not copy context bodies, turn prompts, credentials, workflow policy, or
operator-only entries. Required entries already delivered by a provider
adapter are not listed for redundant MCP acquisition.

Claude receives the bootstrap before explicit appended system context. Codex
receives it before explicit context and the current turn prompt. Both fresh and
resumed provider commands receive the current operation's bootstrap because
the private endpoint and evidence fence rotate per finite operation. The
transient rendered instruction is non-serializable and redacted from launch
debugging; `ContextSnapshot.bootstrap` exposes the typed artifact during the
operation and for the latest settled operation.

This is a deliberately fixed harness contract, not a universal workflow
prompt. It tells the provider where it exists and what it must acquire; only
the separate read evidence proves that the provider MCP client made those
requests.

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
| Roba bootstrap | Prepended to appended system context. | Prepended before explicit context and the current prompt. | Typed operation, authority, manifest, required-acquisition list, and fingerprint are retained in `ContextSnapshot`. |
| Steering | Becomes another provider-native resumed turn. | Becomes another provider-native resumed turn. | Core events record the queued turn boundary. |
| Provider session | Claude session ID and transcript are provider-owned. | Codex thread ID and transcript are provider-owned. | Roba retains only the opaque handle and reported terminal evidence. |
| Private Roba MCP endpoint | Reattached through an owner-private temporary MCP config for every finite operation. | Reattached through command overrides and a child-only bearer environment variable. | Endpoint metadata is non-serializable; credentials are redacted. |
| Provider-native ambient context | Ambient leaves normal discovery enabled. Controlled passes an empty `--setting-sources`, `--strict-mcp-config`, and `--exclude-dynamic-system-prompt-sections`. Dynamic sections are relocated, not removed; managed policy and automatic memory remain unobservable. | Ambient leaves normal discovery enabled. Controlled passes `--ignore-user-config`, `--ignore-rules`, and `memories.use_memories=false`. Project config, `AGENTS.md`, skills, plugins, and MCP remain discoverable. | Exact command mechanics and source-class dispositions are capability-tested. Provider baseline and managed state remain explicitly unobservable. |

Every hot-agent operation creates a new finite `RunSpec` from the same fixed
template. The current adapters therefore repeat all three explicit context
vectors even when resuming the provider session. Characterization tests pin
this behavior so later generation-aware delivery changes cannot be accidental.

## Provider-native sources outside the current plan

Claude Code can load managed policy, command-line settings, local/project/user
settings, `CLAUDE.md`, rules, skills, hooks, MCP servers, plugins, and automatic
memory. Controlled mode mechanically suppresses the normal user, project, and
local setting-source stack plus ambient MCP configuration. It relocates
dynamic system sections into the first user message. It does not select
`--safe-mode`, because that mode also disables MCP and would conflict with the
private Roba callback contract. Native `CLAUDE.md`, rules, skills, plugins, and
hooks may therefore remain discoverable. The adapter also cannot prove managed
policy, automatic memory, or the provider baseline absent. Those source classes
remain retained or unobservable in inspection rather than being reported as
suppressed.

Codex can load built-in instructions, user and trusted-project `config.toml`
layers, global and nested `AGENTS.md`, rules, skills, plugins, MCP servers, and
other per-user state under `CODEX_HOME`. Controlled mode mechanically ignores
the user config and exec-policy rules and disables generated memories. Trusted
project configuration, `AGENTS.md`, skills, plugins, MCP, authentication, and
the provider baseline remain outside that reduction and are reported as such.

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

1. Move the reserved base surface through the same internal contribution
   compiler without making it replaceable.
2. Stop reinjecting unchanged stable context into resumed sessions where the
   provider mechanics prove that omission is safe.
3. Add context linting for duplicate fingerprints, precedence conflicts,
   conflicting instructions, unsafe locators, and excessive prompt weight.
4. Add explicit acknowledgement and gate provider-facing mutation on the
   required evidence policy.
5. Apply the same manifest to parent-spawned Robas without inheriting the
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

- no universal workflow or task-specific system prompt beyond the fixed,
  minimal Roba bootstrap contract;
- no claim that MCP availability means the provider requested context;
- no serialization of prompt bodies, secrets, or provider-private sessions;
- no provider-independent claim of hermetic behavior;
- no workflow, issue, repository, or parent-child policy in `roba-core`;
- no claim that an MCP read means the model acknowledged or followed content;
- no mutation gating or context update API yet.
