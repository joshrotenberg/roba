# roba permissions

roba is **safe by default**: claude can use `Read`, `Glob`, and `Grep`
and nothing else unless you opt into more. This page covers the opt-in
flags, the cross-layer precedence model (CLI > env > profile >
built-in), and how to preview the resolved set before a run. For the
big-picture framing, see the repo [`README.md`](../README.md); for the
matching profile fields, see [`profiles.md`](profiles.md).

## The opt-ins

Safe by default. claude can use `Read`, `Glob`, and `Grep` but
nothing else unless you say so:

```bash
roba "explain this"              # readonly default
roba "..." --writable            # add Edit + Write
roba "..." --allow-tool "Bash(git status)"   # add one specific pattern
roba "..." --deny-tool WebFetch  # block a specific tool
roba "..." --full-auto           # bypass every check (sandbox only)
roba --profile review --show-permissions   # preview the resolved set, then exit
```

Same knobs work as profile fields (`writable = true`,
`allow_tool = [...]`, etc.) so you can codify a project's policy
in `roba.toml` once and not think about it again. See
[`profiles.md`](profiles.md) for the full schema.

## Precedence

When the same permission knob is set in more than one place, the
highest layer wins:

| Layer | Example |
|---|---|
| CLI flag | `--writable`, `--allow-tool Edit` |
| Env var | `ROBA_WRITABLE=1`, `ROBA_ALLOW_TOOL=Edit,Write` |
| Active profile overlay | `[profile.NAME] writable = true` |
| Top-level `roba.toml` | `writable = true` at the file's top level |
| Built-in default | read-only: `Read`, `Glob`, `Grep` only |

`--readonly`, `--writable`, and `--full-auto` are mutually
exclusive across layers (they're presence flags that flip a bool).
The highest layer that sets one wins, and a higher-priority flag
**suppresses** the lower-privilege ones from lower layers:

- `--readonly` suppresses lower-layer `writable = true` and
  `full_auto = true`.
- `--writable` suppresses lower-layer `full_auto = true`.

`--full-auto` beats `--writable` because `apply_permissions`
short-circuits on `full_auto` before inspecting `writable`.

`--readonly` is the explicit name for the built-in default, but it
is now an **active suppressor**: passing `--readonly` on the CLI
cancels a `writable = true` or `full_auto = true` coming from a
profile or env var, so the call stays read-only.

`allow_tool` and `deny_tool` lists **accumulate across layers**.
Across `roba.toml` files, closer-to-cwd entries concat on top of
farther-from-cwd entries, and the active profile's list concats
on top of the top-level list. The CLI (`--allow-tool` /
`--deny-tool`, repeatable) and env (`ROBA_ALLOW_TOOL` /
`ROBA_DENY_TOOL`, comma-separated) each **replace** the resolved
list when set, rather than concatenating with it.

When the same tool ends up in both the allow list and the deny
list, **deny wins**. roba passes both lists through to claude
unchanged; claude is the final arbiter.

## Previewing the resolved set

Because a lower-layer profile can quietly add `writable = true` or
extra allow-list entries, it's easy to fire a prompt assuming a
permission set you didn't actually get. `--show-permissions`
resolves every layer (the same flow a real run uses), prints the
effective allow/deny lists with per-entry provenance, and exits 0
**without calling claude**:

```text
$ roba --profile review --show-permissions
allow:
  Read       [default]
  Glob       [default]
  Grep       [default]
  Edit       [profile.review]
  Write      [profile.review]
deny:
  Bash(rm *) [profile.review]
```

Each tag shows the winning layer: `[default]` for the built-in safe
trio, or `[CLI]` / `[env]` / `[profile.NAME]` / `[config]` for
anything a higher layer contributed. Under `--full-auto` the output
collapses to a single line (`all tools allowed (--full-auto from
...)`), since the resolution is "everything." The preview goes to
stderr, so stdout stays clean.

### Worked example

Profile in `roba.toml`:

```toml
[profile.default]
writable = true
allow_tool = ["Bash(git status)"]
```

| Invocation | Resolved permissions |
|---|---|
| `roba "..."` | writable (Edit, Write) + `Bash(git status)` (auto-applied profile) |
| `roba --full-auto "..."` | full-auto bypasses everything; profile's writable + allow_tool ignored |
| `roba --no-default-profile "..."` | read-only default (Read, Glob, Grep); profile skipped |
| `roba --readonly "..."` | read-only -- `--readonly` suppresses the profile's `writable = true` (and any lower-layer `full_auto`) |
| `roba --allow-tool Edit "..."` | read-only base + `Edit` only; profile's `allow_tool` list is replaced, but `writable = true` from the profile still applies (so Edit, Write are also in) |
