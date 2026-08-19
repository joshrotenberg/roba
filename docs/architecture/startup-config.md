# Provider-neutral startup configuration

`roba run` and `roba serve` share one versioned startup contract. The host
resolves it once before provider launch; a hot `serve` process pins that
resolved snapshot for its lifetime.

## Deterministic initialization

`roba init` creates a conservative `roba.toml` in the effective cwd. The
default selects read-only authority and leaves managed context unselected, so
provider-native ambient behavior remains available:

```bash
roba init
roba init --dry-run
roba init --agent-role roba.repo-worker --prompt roba.issue-worker
```

The dry run and installed file use one canonical renderer. Before installation
Roba validates the document through this same strict schema and catalog
resolver. Installation uses an atomic no-clobber path and refuses when the
directory already contains `roba.toml`, `.roba.toml`, or `.roba/roba.toml`.
It never launches a provider or copies managed catalog bodies into the file.

## Version 1 schema

```toml
version = 1

[agent]
provider = "codex"
model = "provider-model-id"
effort = "high"
instructions = ["Work in small, reviewable steps."]

[execution]
permissions = "workspace_write"
timeout_secs = 900

[session]
mode = "sticky"

[context]
ambient_policy = "controlled"
project = ["Tests are the acceptance boundary."]
agent = "roba.repo-worker"
skills = []
prompts = ["roba.issue-worker"]

[context.builtins]
enabled = true

[[context.definitions]]
kind = "skill"
id = "local.project-conventions"
description = "Repository-specific engineering conventions."
path = ".roba/skills/project-conventions.md"

[extensions.git]
enabled = true
progress_interval_secs = 5
```

The complete commented example is
[`roba-startup.sample.toml`](../../roba-startup.sample.toml). Unknown fields,
unsupported versions, invalid limits, and provider controls that the selected
adapter cannot enforce fail before provider work begins. Provider-private
session ids are CLI-only and are never accepted from or printed in this file.
Session mode is provider-neutral: `sticky` retains validated continuity,
`fresh` starts every admitted operation without it, and phase-one `managed`
retains continuity until an explicit generation-fenced clean rotation. A
`fresh` policy conflicts with `--resume` and fails during resolution.

The managed catalog is resolved and validated at startup. Built-ins are
available by default, but absence of `context.agent` preserves ambient-only
behavior and creates no effective selection. Selecting skills or prompts
requires an agent. Local definitions use exactly one bounded `inline` value or
Markdown `path`; paths resolve relative to the file that declares them and
cannot escape that directory. Definition IDs cannot replace another layer or
the reserved `roba.*` namespace.

The root host passes the same resolved catalog and selection into `roba-mcp`.
Selected prompts become operator-only MCP prompts. The selected agent and
transitive skills become provider-visible context-plan entries without being
copied into provider prompts or serialized run intent. Only an exact
provider-side `context.read` becomes acquisition evidence.

A Git progress interval of `0` disables periodic active-operation sampling
while retaining the admission baseline and final refresh. Context
`ambient_policy` defaults to `ambient`; `controlled` applies the selected
adapter's mechanically tested reduction, while unsupported `hermetic` requests
fail during host construction. Exact retained, suppressed, and unobservable
provider source classes are published through `roba://context`. Strict
unknown-field rejection prevents a plausible-looking future key from being
silently ignored.

## Discovery and precedence

The lowest-priority layer is the user file:

- `$XDG_CONFIG_HOME/roba/roba.toml`, or
- `~/.config/roba/roba.toml` when `XDG_CONFIG_HOME` is unset.

Roba then walks from the effective cwd (`-C` is applied first) to the Git root.
At each directory it recognizes one versioned candidate:

- `roba.toml`;
- `.roba.toml`;
- `.roba/roba.toml`.

Farthest files load first and closer files win scalar conflicts. Instruction,
raw context, selected skill/prompt, disabled-built-in, and definition lists
compose in layer order. Duplicate selected IDs and duplicate definition IDs
fail closed rather than being silently deduplicated or replaced. Two versioned
sibling candidates are an ambiguity error. `--config PATH` uses only one
explicit file; `--no-config` uses built-in defaults. Explicit CLI values
override files, while repeated `--instruction` and `--context` values append
to the declared stack.

Unversioned files are not accepted. Roba never guesses whether a file intended
an older schema or silently translates keys with different semantics. The old
user path `~/.config/roba.toml` is not searched.

The removed Claude-only profiles, shell-expanding aliases, named sessions,
`ROBA_*` overrides, and bundle configuration were deliberately not migrated.
Provider-private resume ids remain an explicit CLI input rather than shareable
project configuration.

## Inspection and provenance

`roba config effective` resolves and validates the same startup stack without
starting a provider. It prints safe TOML by default or a versioned JSON
envelope with `--json`. The result lists loaded files and the winning source
for every scalar; composed lists retain all contributing sources. Managed
catalog output contains selected IDs, resolved transitive skills, origins,
relative source locators, and SHA-256 fingerprints, but never inline or file
bodies. A supplied `--resume` is represented only by `resume_seeded = true`,
never by its opaque provider id.

The effective context view also includes deterministic diagnostics for the
declared plan. Warnings cover duplicate safe fingerprints, bounded directive
conflicts, prose authority mismatches, repeated stable delivery, and excessive
eager material. Hard locator or required-delivery findings make startup fail
before provider work. Diagnostics contain IDs and safe provenance, never
bodies, secrets, or raw locators.

```bash
roba -C /path/to/repo config effective
roba -C /path/to/repo config effective --provider claude --read-only --json
```

`roba config survey` is the inspectable input boundary for future
provider-assisted tuning. It validates the same startup host, then emits:

- the safe provider, authority, limit, ambient-policy, catalog, context
  manifest, diagnostic, extension, and provenance views;
- the canonical working directory and nearest `.git` repository boundary;
- a fixed, nonrecursive list of recognized guidance, documentation, package,
  automation, source, test, workflow, and `.roba` markers at the nearest
  repository root, or the effective cwd outside a repository;
- the explicit fact that file contents were not included.

The first survey schema never recursively walks the workspace, reads file
bodies, executes Git, starts a provider, or writes configuration. Unknown
files are absent rather than heuristically classified. Present symlinks and
wrong-type markers are reported as bounded omissions. Safe startup evidence
has a 1 MiB serialized ceiling, reported with its observed size. This packet is a
prerequisite for `init --survey` or `config tune`, not a model proposal itself.

```bash
roba -C /path/to/repo config survey
roba -C /path/to/repo config survey --json
```

`roba config propose` is the first provider-assisted consumer of this packet.
It creates one purpose-built proposal host with these fixed properties:

- fresh provider session;
- read-only execution authority;
- the provider's mechanically enforced `controlled` ambient-context posture;
- no Git or other optional extension capability;
- the exact survey as a mandatory generation-fenced context entry;
- one provider-only `config.propose` tool with a strict input schema.

The command retains resolved provider, model, effort, timeout, turn, and cost
limits, but it does not carry standing instructions, explicit project/run
context, a resume seed, managed role selection, or extension tools into the
proposal operation. A successful result requires mechanical evidence that the
provider read the survey and submitted exactly one typed candidate. Prose-only
answers, invalid catalog IDs, repeated submissions, and unsupported values fail
closed.

The first candidate schema is intentionally conservative. It can propose a
provider, model, effort, read-only or workspace-write authority, ambient or
controlled context, shipped catalog IDs, and Git activation. It cannot propose
`full_auto`, `hermetic`, limits, arbitrary instructions, inline definitions, or
file paths. Only shipped built-in catalog IDs are eligible because the preview
is rendered as a standalone strict startup document.

Plain output is canonical TOML with empty success stderr; provider prose is not
written directly to a terminal. `--json` returns the typed proposal, rationale,
rendered document, actual proposal-execution posture, provider-reported
telemetry, and survey-read evidence in a versioned envelope. This is always a
preview: it never edits or replaces a discovered config. A later tuning slice
must define semantic merge, diff, confirmation, and atomic application rather
than treating this safe subset as a lossless replacement for content-bearing
configuration.

```bash
roba -C /path/to/repo config propose --provider codex
roba -C /path/to/repo config propose --provider claude --json
```

Runtime startup files are read-only inputs. Aside from the explicit no-clobber
`roba init` command, Roba does not write discovered config, extension state,
credentials, task history, or provider-private session data into them.
