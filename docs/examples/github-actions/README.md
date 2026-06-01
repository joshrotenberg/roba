# roba in GitHub Actions

Example workflows for running `roba` in CI. Each file here is a
starting point, not a drop-in tool -- read it, understand what it
spends, and adapt it to your repo before relying on it.

## Examples

| File | What it does |
|---|---|
| [`pr-review.yml`](pr-review.yml) | Runs `roba` on a PR's diff and posts the result as a PR comment, on open and on push |

## Before you wire one up

### Auth

`roba` shells out to the `claude` binary, which needs an Anthropic API
key. Set `ANTHROPIC_API_KEY` in your repo (or org) secrets and pass it
through to the step's environment, as the example does. The default
`GITHUB_TOKEN` covers posting comments; no extra GitHub auth is
needed for the example's `gh pr comment` call.

### Cost discipline

A PR-triggered job runs on every open and every push to an open PR,
so the spend adds up. The example is built around keeping that
bounded:

- **Cheap model by default.** `--model haiku` is the example's
  default. Reach for a bigger model only where the review quality
  actually justifies it.
- **Short, scoped prompts.** A focused prompt costs less and produces
  a more useful comment than "review everything."
- **Smoke a subset first.** Before pointing a workflow at a busy repo,
  run it against a handful of PRs and look at the real spend. Then
  decide whether to gate it -- only on a label, only when certain
  paths change, or only on non-draft PRs.

### What's intentionally not here

These examples stop at *producing a review*. They do not:

- **Auto-merge.** Whether a green check should merge a PR is a
  policy choice that belongs to you, not to an example file.
- **Auto-fix.** Letting an agent push commits back to a PR branch is
  a bigger trust and permission decision than read-and-comment, and
  it needs `--writable` (or more) plus write-scoped tokens. Out of
  scope here on purpose.

If you want either, you're building a tool, not copying an example --
make those calls deliberately.
