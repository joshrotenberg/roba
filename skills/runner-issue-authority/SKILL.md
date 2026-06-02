---
name: runner-issue-authority
description: When dispatched with an issue number, the runner's authoritative source for what to do is `gh issue view <N>`, NOT the orchestrator's paraphrase. The orchestrator's invocation provides the issue number and any explicit overrides; the runner self-fetches. This keeps state in GitHub (where it belongs) and avoids paraphrase drift.
---

# Authority for task content (runner)

When dispatched with an issue number, **`gh issue view <N>` is your
authoritative source for what the task is.** This is non-negotiable.

## Rules

- **Always fetch first.** Your first action when dispatched with an
  issue number is `gh issue view <N>`. Do it before composing the
  prompt, even if the orchestrator's invocation includes a paraphrase
  or summary of the issue body.
- **The orchestrator does NOT paste the issue body.** That's an
  anti-pattern -- it duplicates state that lives in GitHub, risks
  paraphrase drift, and violates the state-externalization corollary
  (the issue is the durable source; conversation is transient).
- **The orchestrator's invocation provides three things:**
  - (a) the issue number (and optionally repo path / owner)
  - (b) explicit constraints / overrides / scope-narrowers that
    don't appear in the issue
  - (c) sometimes a pointer like "focus on X" or "skip Y."
- **Synthesize:** issue body (authoritative) + orchestrator's
  constraints (local overrides). If they conflict, the orchestrator's
  explicit override wins -- the issue is the spec; the orchestrator
  is the local interpreter for this particular dispatch.

## Dispatch shape you'll see

The orchestrator sends a minimal structured directive (per the
orchestrator's dispatch-format discipline):

```
implement #<N> in <repo-path>

constraints:
- <override or scope-narrower>
- <override or scope-narrower>
```

- First line is the directive (`implement #N`, `fix CI in PR #N`,
  etc.).
- `<repo-path>` is optional when the issue is in the current cwd's
  project.
- `constraints:` is optional. The orchestrator includes it only
  when they have explicit overrides; if the directive is bare, the
  issue body is the spec.

## Why this matters

If the orchestrator paraphrases the issue into the dispatch prompt:

- The dispatch prompt bloats (orchestrator context burns tokens to
  write the paraphrase)
- Drift risk: orchestrator's summary diverges from the actual issue
- State duplication: same content in GitHub AND in the dispatch
  prompt
- Violates the state-externalization corollary: state should live in
  durable stores (GitHub issues), not transit through the
  conversation

The cleanest contract: the issue body is in GitHub; the orchestrator
gives you a pointer; you fetch.

## Related

- [`draft-pr-first`](../draft-pr-first/SKILL.md) -- the lifecycle
  the runner follows after fetching the issue.
- [`roba-orchestration-prompt`](../roba-orchestration-prompt/SKILL.md)
  -- the prompt-composition template the runner uses.
