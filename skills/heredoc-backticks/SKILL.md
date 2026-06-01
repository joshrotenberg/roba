---
name: heredoc-backticks
description: When piping markdown into `gh issue create --body "$(cat <<'EOF' ... EOF)"` (or any single-quoted heredoc), use literal backticks. Do NOT escape them -- escapes survive into the output and break the markdown.
---

# Heredoc backticks

When piping markdown into `gh issue create --body "$(cat <<'EOF'
... EOF)"` or `gh pr create --body "$(cat <<'EOF' ... EOF)"` (or
any single-quoted heredoc), use **literal backticks**. Do NOT write
the backslash-escaped form.

## Why

A single-quoted heredoc delimiter (`<<'EOF'`) already disables
variable, command, and history expansion in the body. Backslash-
backtick is not interpreted by the shell -- it passes through as
the literal two-character sequence `\` + `` ` ``.

GitHub's markdown renderer then treats the backslash as an escape,
converting each `\`` into a literal backtick *character* (not a code-
fence delimiter). Result: code fences fail to open/close, the
TOML/code block renders as plain prose, and any `#` inside the
unclosed block becomes an H1.

Caught on roba 2026-05-27 after a screenshot showed issue #1's
schema block rendered as broken text.

## How to apply

In a `<<'EOF'` heredoc, write fences as:

    ```toml
    ...
    ```

and inline code as `` `--flag` `` -- no backslashes anywhere. Same
for any other text that the shell would normally interpret (`$VAR`,
`` `cmd` ``, `\n`): inside single-quoted EOF, all of it is literal
already.

## When you'd actually need escaping

If the delimiter is *unquoted* (`<<EOF` instead of `<<'EOF'`),
backticks WOULD do command substitution and would need escaping.
But prefer single-quoted EOF and avoid that situation entirely:

```bash
gh issue create --title "..." --body "$(cat <<'EOF'
markdown body with literal `backticks` and ```code fences```
EOF
)"
```

## Related

- [`roba-orchestration-prompt`](../roba-orchestration-prompt/SKILL.md)
  -- the orchestration loop pipes a lot of markdown into
  `gh pr create`; this skill keeps those bodies rendering correctly.
