# Provider-neutral startup configuration

`roba run` and `roba serve` share one versioned startup contract. The host
resolves it once before provider launch; a hot `serve` process pins that
resolved snapshot for its lifetime.

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
project = ["Tests are the acceptance boundary."]

[extensions.git]
enabled = true
```

The complete commented example is
[`roba-startup.sample.toml`](../../roba-startup.sample.toml). Unknown fields,
unsupported versions, invalid limits, and provider controls that the selected
adapter cannot enforce fail before provider work begins. Provider-private
session ids are CLI-only and are never accepted from or printed in this file.

The schema contains only behavior Roba can enforce today. In particular, a
context isolation `mode` and Git progress polling interval are not accepted
until those capabilities ship. Strict unknown-field rejection prevents a
plausible-looking future key from being silently ignored.

## Discovery and precedence

The lowest-priority layer is the user file:

- `$XDG_CONFIG_HOME/roba/roba.toml`, or
- `~/.config/roba/roba.toml` when `XDG_CONFIG_HOME` is unset.

Roba then walks from the effective cwd (`-C` is applied first) to the Git root.
At each directory it recognizes one versioned candidate:

- `roba.toml`;
- `.roba.toml`;
- `.roba/roba.toml`.

Farthest files load first and closer files win scalar conflicts. Instruction
and context lists compose in layer order. Two versioned sibling candidates are
an ambiguity error. `--config PATH` uses only one explicit file; `--no-config`
uses built-in defaults. Explicit CLI values override files, while repeated
`--instruction` and `--context` values append to the declared stack.

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
for every scalar; composed lists retain all contributing sources. A supplied
`--resume` is represented only by `resume_seeded = true`, never by its opaque
provider id.

```bash
roba -C /path/to/repo config effective
roba -C /path/to/repo config effective --provider claude --read-only --json
```

Startup files are read-only inputs. Roba does not write discovered config,
extension state, credentials, task history, or provider-private session data
into them.
