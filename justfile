# Show available recipes.
default:
    @just --list

# One-time per clone: enable the repository's formatting hook.
setup:
    git config core.hooksPath .githooks
    @echo "git hooks enabled: .githooks/pre-commit"

# Fast deterministic tests.
test:
    cargo test --workspace --lib --all-features
    cargo test --test cli --all-features

# Full deterministic workspace coverage.
test-all:
    cargo test --workspace --all-features

# Pre-commit gates.
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo build --workspace --all-features

# All paid provider smoke tests. Requires authenticated provider binaries.
live:
    cargo test --test live -- --ignored --nocapture --test-threads=1

# One real Claude boundary smoke test.
live-claude:
    cargo test --test live -- --ignored --nocapture live_claude

# One real Codex boundary smoke test.
live-codex:
    cargo test --test live -- --ignored --nocapture live_codex
