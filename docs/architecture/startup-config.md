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

```bash
roba -C /path/to/repo config effective
roba -C /path/to/repo config effective --provider claude --read-only --json
```

Runtime startup files are read-only inputs. Aside from the explicit no-clobber
`roba init` command, Roba does not write discovered config, extension state,
credentials, task history, or provider-private session data into them.
