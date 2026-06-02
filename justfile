# Show available recipes
default:
    @just --list

# Unit + mechanical CLI tests. Fast, no claude calls. Run often.
test:
    cargo test --lib --all-features
    cargo test --test cli --all-features

# Pre-commit gates. Run before pushing.
check:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings

# Pre-flight: confirm claude is on PATH (live tests need this).
preflight:
    @claude --version > /dev/null 2>&1 && echo "claude: ok" || (echo "claude not found or not authenticated" && exit 1)

# Full live test suite. Calls real claude; haiku default keeps cost low.
live: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2

# Smoke: the minimum to know roba's main paths still work. A few tests, fast and cheap.
live-smoke: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 \
        live_smoke live_session_continue live_output_json

# Category runners. The filter is a substring match on the test name.
live-output: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 live_output

live-session: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 live_session

live-stream: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 live_stream

live-compose: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 live_compose

# Categories for future tests filed under #22. Placeholders today.
live-perms: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 live_perms

live-profile: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 live_profile

live-env: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 live_env

live-subcmd: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 live_subcmd

# Run any single category by prefix without a dedicated target.
# Example: just live-category perms  ->  filters on `live_perms`.
live-category category: preflight
    cargo test --test live -- --ignored --nocapture --test-threads=2 live_{{category}}

# Spend rollup. #11 (cost in dollars) is merged, so `roba cost` reports
# token + USD totals across history directly. Run after a live sweep to
# see what it cost.
live-cost:
    cargo run --quiet --all-features -- cost
