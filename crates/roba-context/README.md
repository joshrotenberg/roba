# roba-context

`roba-context` is the data-oriented catalog for context that Roba manages.
It defines strict agent, skill, and prompt artifacts; resolves bounded inline
or repository-local Markdown sources; records content-free provenance and
fingerprints; and computes deterministic selections.

The crate does not own MCP routers, provider processes, hot-agent lifecycle,
startup configuration, scheduling, or authority. The root host uses it to
resolve strict layered startup definitions and safe effective provenance.
`roba-mcp` will consume that resolved catalog in the next layer and compile
selected artifacts into its immutable, generation-fenced context plan.

The initial built-in catalog is intentionally small:

- `roba.repo-worker` -- a bounded repository-worker role;
- `roba.repository-change` -- a reusable repository-change method;
- `roba.issue-worker` -- a parameterized issue-work directive.

Catalog availability is not prompt injection and is not evidence that a
provider read or followed an artifact.
