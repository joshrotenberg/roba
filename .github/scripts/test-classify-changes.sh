#!/usr/bin/env bash

set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
classifier="$script_dir/classify-changes.sh"

assert_classification() {
  local expected=$1
  shift

  local actual
  actual=$(printf '%s\0' "$@" | "$classifier")
  if [[ "$actual" != "$expected" ]]; then
    printf 'classification mismatch\nexpected:\n%s\nactual:\n%s\n' \
      "$expected" "$actual" >&2
    return 1
  fi
}

assert_classification \
  $'docs_only=true\nmarkdown_changed=true' \
  README.md docs/architecture/core.md

assert_classification \
  $'docs_only=true\nmarkdown_changed=true' \
  .mdbook-lint.toml

assert_classification \
  $'docs_only=false\nmarkdown_changed=true' \
  README.md crates/roba-core/src/lib.rs

assert_classification \
  $'docs_only=false\nmarkdown_changed=false' \
  Cargo.toml .github/workflows/ci.yml

assert_classification \
  $'docs_only=true\nmarkdown_changed=true' \
  $'docs/a file with spaces.md' $'docs/a file with a\nnewline.md'

actual=$("$classifier" </dev/null)
expected=$'docs_only=false\nmarkdown_changed=false'
if [[ "$actual" != "$expected" ]]; then
  printf 'empty classification mismatch\nexpected:\n%s\nactual:\n%s\n' \
    "$expected" "$actual" >&2
  exit 1
fi
