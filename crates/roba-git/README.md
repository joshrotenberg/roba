# roba-git

`roba-git` is an optional, workspace-scoped MCP fragment for Roba. It fixes
one repository at construction and exposes a typed snapshot through both a
tool and a resource. A separately authorized control projection can stage all
current changes and returns a before/after receipt plus the exact index tree.

Surface by projection:

| Capability | Read-only control | Writable control | Provider |
|---|---:|---:|---:|
| `git.snapshot` | yes | yes | yes |
| `roba://git/workspace` | yes | yes | yes |
| `git.stage_all` | no | yes | no |

`GitWorkspace::discover` selects the nearest ancestor with a `.git` directory
or worktree file. `GitWorkspace::extension` returns one `AgentExtension` whose
control and provider fragments share that captured service. Roba enables it
explicitly with `roba run --git` or `roba serve --git`; without that flag the
base MCP discovery is unchanged.

The provider projection is deliberately read-only. Git mutation can execute
repository-configured filters outside a provider sandbox, and it can take
longer than the operation-scoped callback endpoint can currently promise to
drain. Raw Git remains the escape hatch when this narrow workflow is not the
right abstraction.

The service never reads the process working directory after construction and
never accepts a caller-selected repository path. Read operations disable
Git's optional locks and configured filesystem monitor. All Git commands are
bounded. `git.stage_all` serializes mutations, refuses unresolved conflicts or
nothing-to-stage requests, and records the resulting index-tree object id.

Git state is read on demand and is not injected into an agent prompt. Provider
approval names exactly `git.snapshot`; turn admission, agent controls, and
staging are structurally absent from the provider fragment. Raw Git remains
available when this narrow workflow does not express the required operation.
