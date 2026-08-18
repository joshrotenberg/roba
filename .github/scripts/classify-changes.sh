#!/usr/bin/env bash

set -euo pipefail

docs_only=true
markdown_changed=false
path_count=0

# Read NUL-delimited paths so whitespace and newlines in filenames are safe.
while IFS= read -r -d '' path; do
  path_count=$((path_count + 1))

  case "$path" in
    *.md | .mdbook-lint.toml)
      markdown_changed=true
      ;;
    *)
      docs_only=false
      ;;
  esac
done

# An empty or indeterminate diff must run the full suite.
if [[ "$path_count" -eq 0 ]]; then
  docs_only=false
fi

printf 'docs_only=%s\n' "$docs_only"
printf 'markdown_changed=%s\n' "$markdown_changed"
