//! Output-side helpers: formatters for the cost footer, truncation,
//! refusal heuristics, code-block extraction.
//!
//! All pure functions -- safe to unit-test without claude calls.

use claude_wrapper::types::QueryResult;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;

use crate::cli::AskArgs;

/// True when stderr is a TTY and the user hasn't opted out via --quiet.
/// Gates the cost footer, refusal warning, and any other stderr noise.
pub fn should_show_footer(args: &AskArgs) -> bool {
    !args.quiet && std::io::stderr().is_terminal()
}

/// Heuristic: does this answer look like a refusal? Matches a small
/// set of leading phrases case-insensitively; ignores phrases that
/// appear mid-paragraph since those are usually the model discussing
/// refusals, not actually refusing.
pub fn looks_like_refusal(text: &str) -> bool {
    let lower = text.trim_start().to_lowercase();
    const MARKERS: &[&str] = &[
        "i can't help",
        "i cannot help",
        "i can't assist",
        "i'm not able to",
        "i am not able to",
        "that's outside",
        "that is outside",
        "i won't",
        "i will not",
        "sorry, i can't",
        "sorry, i cannot",
        "i don't think i can",
        "i'm set up for",
        "i am set up for",
        "i'm designed to",
        "unfortunately, i can't",
        "i'm not going to",
    ];
    MARKERS.iter().any(|m| lower.starts_with(m))
}

/// `.json` extension check for --out format inference.
pub fn path_is_json(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("json")
}

/// Extract fenced code blocks from `text`. Blocks open with a line
/// starting with three backticks (optionally followed by a language
/// info string) and close with the same. With `lang_filter`,
/// non-matching blocks are dropped. Unclosed blocks are discarded.
pub fn extract_code_blocks(text: &str, lang_filter: Option<&str>) -> String {
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_block = false;
    let mut block_lang: String = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("```") {
            if in_block {
                let keep = match lang_filter {
                    None => true,
                    Some(want) => block_lang.eq_ignore_ascii_case(want),
                };
                if keep {
                    blocks.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                in_block = false;
                block_lang.clear();
            } else {
                in_block = true;
                block_lang = rest.trim().to_string();
            }
        } else if in_block {
            current.push_str(line);
            current.push('\n');
        }
    }
    blocks.join("\n")
}

/// Single-line footer summarizing a [`QueryResult`]: tokens, cost,
/// duration, session id (first 8 chars).
///
/// The dollar figure prefers claude's own authoritative `total_cost_usd`
/// when present; otherwise, if `rates` is supplied and the call's model
/// is in the table, it is computed from the rate table. `no_dollars`
/// suppresses the figure entirely.
pub fn format_footer(
    r: &QueryResult,
    rates: Option<&crate::rates::Rates>,
    no_dollars: bool,
) -> String {
    let mut parts = Vec::new();
    if let Some((input, output)) = extract_tokens(&r.extra) {
        parts.push(format!(
            "tokens {}/{}",
            format_count(input),
            format_count(output)
        ));
    }
    if !no_dollars && let Some(cost) = footer_cost(r, rates) {
        parts.push(format!("${cost:.4}"));
    }
    if let Some(ms) = r.duration_ms {
        parts.push(format_duration(ms));
    }
    let id = r.session_id.get(..8).unwrap_or(&r.session_id);
    parts.push(format!("session {id}"));
    parts.join(" . ")
}

/// Resolve the footer's dollar figure: claude's authoritative
/// `total_cost_usd` first, then a rate-table computation from the
/// model + usage breakdown in `extra`. `None` when neither is
/// available (e.g. a subscription run with no model in the table).
fn footer_cost(r: &QueryResult, rates: Option<&crate::rates::Rates>) -> Option<f64> {
    if let Some(c) = r.cost_usd {
        return Some(c);
    }
    let rates = rates?;
    let model = extract_model(&r.extra)?;
    let (input, output, cache_read, cache_write) = extract_full_usage(&r.extra)?;
    rates.cost_usd(&model, input, output, cache_read, cache_write)
}

/// Best-effort model id from a result's `extra`: a top-level `model`
/// scalar, else the single key of a `modelUsage` map.
fn extract_model(extra: &HashMap<String, serde_json::Value>) -> Option<String> {
    if let Some(m) = extra.get("model").and_then(|v| v.as_str()) {
        return Some(m.to_string());
    }
    extra
        .get("modelUsage")
        .and_then(|v| v.as_object())
        .and_then(|m| m.keys().next().cloned())
}

/// Pull the full `(input, output, cache_read, cache_write)` token
/// breakdown out of `extra["usage"]`. Returns `None` only when the
/// `usage` object itself is missing; individual buckets default to 0.
fn extract_full_usage(extra: &HashMap<String, serde_json::Value>) -> Option<(u64, u64, u64, u64)> {
    let usage = extra.get("usage")?;
    let g = |k: &str| usage.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    Some((
        g("input_tokens"),
        g("output_tokens"),
        g("cache_read_input_tokens"),
        g("cache_creation_input_tokens"),
    ))
}

/// Pull `(input_tokens, output_tokens)` out of the `usage` field of
/// a [`QueryResult::extra`]. Returns `None` if the shape doesn't
/// match -- claude's JSON is not strictly typed at this layer.
pub fn extract_tokens(extra: &HashMap<String, serde_json::Value>) -> Option<(u64, u64)> {
    let usage = extra.get("usage")?;
    let input = usage.get("input_tokens")?.as_u64()?;
    let output = usage.get("output_tokens")?.as_u64()?;
    Some((input, output))
}

/// Render a count compactly: `42`, `1.2k`, `3.4M`.
pub fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}

/// Render an ms duration as `4.3s` or `1m23s`.
pub fn format_duration(ms: u64) -> String {
    let secs = ms as f64 / 1000.0;
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let m = (secs / 60.0) as u64;
        let s = secs - (m as f64) * 60.0;
        format!("{m}m{s:.0}s")
    }
}

/// Trim an ISO timestamp string down to "YYYY-MM-DD HH:MM" for the
/// history table and picker. Returns `None` if the input is shorter
/// than 16 chars.
pub fn format_timestamp(raw: &str) -> Option<String> {
    let truncated = raw.get(..16)?;
    Some(truncated.replace('T', " "))
}

/// Truncate `s` to at most `max` characters, appending `...` when cut.
pub fn truncate_arg(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
        out.push_str("...");
        out
    }
}

/// Format the rollup line: `Read x3, Edit x2, Bash x1` sorted by
/// count descending, then name ascending.
pub fn format_tool_summary(counts: &HashMap<String, usize>) -> String {
    let mut sorted: Vec<(&String, &usize)> = counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    sorted
        .iter()
        .map(|(k, v)| format!("{k} x{v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Pick a primary arg from a tool's input and format the inline
/// indicator line: `Read(file.rs)`, `Bash(git status)`.
pub fn summarize_tool(name: &str, input: &serde_json::Value) -> String {
    let primary = ["file_path", "command", "pattern", "path", "url", "query"]
        .iter()
        .find_map(|k| input.get(k).and_then(|v| v.as_str()));
    match primary {
        Some(arg) => format!("{name}({})", truncate_arg(arg, 60)),
        None => name.to_string(),
    }
}

/// Render the effective permission set for `--show-permissions`.
///
/// Reads the resolved [`AskArgs`] (after the full CLI > env > profile
/// merge) plus the per-entry provenance fields and produces a
/// machine-readable, human-skimmable block. The always-on safe
/// defaults (Read, Glob, Grep) are tagged `[default]`; everything
/// else carries the layer that contributed it (`CLI`, `env`,
/// `profile.<name>`, or `config`).
///
/// In `--full-auto` mode there are no allow/deny lists -- the result
/// is a single line stating that everything is allowed and from where.
/// The returned string has no trailing newline.
pub fn format_permissions(args: &AskArgs) -> String {
    if args.full_auto {
        let src = args.full_auto_source.as_deref().unwrap_or("CLI");
        return format!("all tools allowed (--full-auto from {src})");
    }

    let mut out = String::new();

    // Show --permission-mode when explicitly set (not the implicit default).
    if let Some(mode) = args.permission_mode {
        #[allow(deprecated)]
        let mode_str = match mode {
            crate::cli::PermMode::Default => "default",
            crate::cli::PermMode::AcceptEdits => "acceptEdits",
            crate::cli::PermMode::DontAsk => "dontAsk",
            crate::cli::PermMode::Plan => "plan",
            crate::cli::PermMode::Auto => "auto",
            crate::cli::PermMode::BypassPermissions => "bypassPermissions",
        };
        let src = args.permission_mode_source.as_deref().unwrap_or("CLI");
        out.push_str(&format!("permission-mode: {mode_str} [{src}]\n"));
    }

    // (name, source) pairs in display order.
    let mut allow: Vec<(String, String)> = vec![
        ("Read".to_string(), "default".to_string()),
        ("Glob".to_string(), "default".to_string()),
        ("Grep".to_string(), "default".to_string()),
    ];
    if args.writable {
        let src = args.writable_source.as_deref().unwrap_or("default");
        allow.push(("Edit".to_string(), src.to_string()));
        allow.push(("Write".to_string(), src.to_string()));
    }
    for (i, tool) in args.allow_tool.iter().enumerate() {
        let src = args
            .allow_tool_sources
            .get(i)
            .map(String::as_str)
            .unwrap_or("default");
        allow.push((tool.clone(), src.to_string()));
    }

    let deny: Vec<(String, String)> = args
        .deny_tool
        .iter()
        .enumerate()
        .map(|(i, tool)| {
            let src = args
                .deny_tool_sources
                .get(i)
                .map(String::as_str)
                .unwrap_or("default");
            (tool.clone(), src.to_string())
        })
        .collect();

    // Pad names to a uniform column so the [source] tags line up.
    // Width spans both lists so allow/deny share one column.
    let width = allow
        .iter()
        .chain(deny.iter())
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0)
        + 1;

    out.push_str("allow:\n");
    for (name, src) in &allow {
        out.push_str(&format!("  {name:<width$}[{src}]\n"));
    }
    if !deny.is_empty() {
        out.push_str("deny:\n");
        for (name, src) in &deny {
            out.push_str(&format!("  {name:<width$}[{src}]\n"));
        }
    }
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_all_blocks_when_no_filter() {
        let md = "intro\n```\nplain\n```\nmiddle\n```rust\nfn x() {}\n```\nend";
        assert_eq!(extract_code_blocks(md, None), "plain\n\nfn x() {}\n");
    }

    #[test]
    fn extract_filters_by_language() {
        let md = "```python\np = 1\n```\n```rust\nlet r = 2;\n```";
        assert_eq!(extract_code_blocks(md, Some("rust")), "let r = 2;\n");
    }

    #[test]
    fn extract_lang_filter_is_case_insensitive() {
        let md = "```Rust\nfn x() {}\n```";
        assert_eq!(extract_code_blocks(md, Some("rust")), "fn x() {}\n");
    }

    #[test]
    fn extract_returns_empty_when_no_blocks_match() {
        let md = "```python\npass\n```";
        assert_eq!(extract_code_blocks(md, Some("rust")), "");
    }

    #[test]
    fn extract_returns_empty_for_no_code_text() {
        assert_eq!(extract_code_blocks("just prose, nothing fenced", None), "");
    }

    #[test]
    fn extract_unclosed_block_is_dropped() {
        let md = "```rust\nfn open() {\n";
        assert_eq!(extract_code_blocks(md, None), "");
    }

    #[test]
    fn format_count_under_1k_is_plain() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(7), "7");
        assert_eq!(format_count(999), "999");
    }

    #[test]
    fn format_count_thousand_scale() {
        assert_eq!(format_count(1_000), "1.0k");
        assert_eq!(format_count(1_234), "1.2k");
        assert_eq!(format_count(999_999), "1000.0k");
    }

    #[test]
    fn format_count_million_scale() {
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(1_500_000), "1.5M");
    }

    #[test]
    fn extract_tokens_reads_nested_usage_keys() {
        let mut extra = HashMap::new();
        extra.insert(
            "usage".to_string(),
            serde_json::json!({"input_tokens": 42, "output_tokens": 7}),
        );
        assert_eq!(extract_tokens(&extra), Some((42, 7)));
    }

    #[test]
    fn extract_tokens_returns_none_when_usage_missing() {
        let extra = HashMap::new();
        assert_eq!(extract_tokens(&extra), None);
    }

    #[test]
    fn extract_tokens_returns_none_on_wrong_shape() {
        let mut extra = HashMap::new();
        extra.insert("usage".to_string(), serde_json::json!("string instead"));
        assert_eq!(extract_tokens(&extra), None);
    }

    #[test]
    fn refusal_detects_common_phrases() {
        assert!(looks_like_refusal("I can't help with that."));
        assert!(looks_like_refusal("I'm not able to do this."));
        assert!(looks_like_refusal(
            "That's outside what I can help with here."
        ));
        assert!(looks_like_refusal("Sorry, I can't assist with that."));
        assert!(looks_like_refusal(
            "I'm set up for software work in this repo."
        ));
    }

    #[test]
    fn refusal_is_case_insensitive() {
        assert!(looks_like_refusal("i can't help"));
        assert!(looks_like_refusal("I CAN'T HELP"));
        assert!(looks_like_refusal("That's Outside what I do"));
    }

    #[test]
    fn refusal_tolerates_leading_whitespace() {
        assert!(looks_like_refusal("   I can't help with that"));
        assert!(looks_like_refusal("\n\nThat's outside"));
    }

    #[test]
    fn refusal_does_not_match_normal_answers() {
        assert!(!looks_like_refusal("Hello!"));
        assert!(!looks_like_refusal("Here is the answer to your question"));
        assert!(!looks_like_refusal("The capital of France is Paris."));
        assert!(!looks_like_refusal("I can help with that:"));
    }

    #[test]
    fn refusal_only_matches_leading_phrase() {
        assert!(!looks_like_refusal(
            "Yes, here's how. Note that I can't help with the part about X."
        ));
    }

    #[test]
    fn truncate_arg_preserves_short_strings() {
        assert_eq!(truncate_arg("hello", 60), "hello");
    }

    #[test]
    fn truncate_arg_cuts_long_strings() {
        let s = "a".repeat(100);
        let out = truncate_arg(&s, 20);
        assert_eq!(out.len(), 20);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn format_duration_under_one_minute_uses_seconds() {
        assert_eq!(format_duration(0), "0.0s");
        assert_eq!(format_duration(500), "0.5s");
        assert_eq!(format_duration(4_300), "4.3s");
        assert_eq!(format_duration(59_900), "59.9s");
    }

    #[test]
    fn format_duration_one_minute_and_over() {
        assert_eq!(format_duration(60_000), "1m0s");
        assert_eq!(format_duration(83_000), "1m23s");
        assert_eq!(format_duration(125_500), "2m6s");
    }

    #[test]
    fn format_timestamp_keeps_date_and_hhmm() {
        assert_eq!(
            format_timestamp("2026-05-21T16:01:00.123Z"),
            Some("2026-05-21 16:01".to_string())
        );
    }

    #[test]
    fn format_timestamp_replaces_t_separator() {
        let out = format_timestamp("2026-05-21T16:01:00Z").unwrap();
        assert!(!out.contains('T'));
        assert!(out.contains(' '));
    }

    #[test]
    fn format_timestamp_returns_none_when_too_short() {
        assert_eq!(format_timestamp("2026-05-21"), None);
    }

    #[test]
    fn format_footer_full_record() {
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "ok",
            "session_id": "abc12345-rest-of-id",
            "total_cost_usd": 0.0192,
            "duration_ms": 1834,
            "num_turns": 1,
            "is_error": false,
            "usage": {"input_tokens": 6, "output_tokens": 14},
        }))
        .expect("build QueryResult fixture");
        let footer = format_footer(&result, None, false);
        assert!(footer.contains("tokens 6/14"));
        // Authoritative claude cost is shown as a bare `$X` segment.
        assert!(footer.contains("$0.0192"));
        assert!(footer.contains("1.8s"));
        assert!(footer.contains("session abc12345"));
    }

    #[test]
    fn format_footer_no_dollars_suppresses_cost() {
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "ok",
            "session_id": "abc12345-rest-of-id",
            "total_cost_usd": 0.0192,
            "duration_ms": 1834,
            "is_error": false,
            "usage": {"input_tokens": 6, "output_tokens": 14},
        }))
        .expect("build QueryResult fixture");
        let footer = format_footer(&result, None, true);
        assert!(footer.contains("tokens 6/14"));
        assert!(
            !footer.contains('$'),
            "no_dollars must omit the cost: {footer}"
        );
    }

    #[test]
    fn format_footer_computes_cost_from_rates_when_claude_omits_it() {
        // No total_cost_usd (subscription-style run), but a known model
        // + usage breakdown in extra lets the rate table fill it in.
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "ok",
            "session_id": "deadbeef",
            "is_error": false,
            "model": "claude-sonnet-4-6",
            "usage": {"input_tokens": 1_000_000, "output_tokens": 0,
                      "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0},
        }))
        .expect("build QueryResult fixture");
        let rates = crate::rates::Rates::bundled().unwrap();
        let footer = format_footer(&result, Some(&rates), false);
        // 1M sonnet input @ $3/MTok = $3.0000.
        assert!(footer.contains("$3.0000"), "got: {footer}");
    }

    #[test]
    fn format_footer_rates_fallback_unknown_model_omits_cost() {
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "ok",
            "session_id": "deadbeef",
            "is_error": false,
            "model": "some-other-llm",
            "usage": {"input_tokens": 100, "output_tokens": 50},
        }))
        .expect("build QueryResult fixture");
        let rates = crate::rates::Rates::bundled().unwrap();
        let footer = format_footer(&result, Some(&rates), false);
        assert!(
            !footer.contains('$'),
            "unknown model must omit cost: {footer}"
        );
    }

    #[test]
    fn format_footer_drops_missing_segments() {
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "ok",
            "session_id": "deadbeef",
            "is_error": false,
        }))
        .expect("build QueryResult fixture");
        let footer = format_footer(&result, None, false);
        assert!(!footer.contains("tokens"));
        assert!(!footer.contains('$'));
        assert!(footer.contains("session deadbeef"));
    }

    #[test]
    fn summarize_tool_uses_primary_arg() {
        let input = serde_json::json!({"file_path": "/tmp/foo.rs"});
        assert_eq!(summarize_tool("Read", &input), "Read(/tmp/foo.rs)");
    }

    #[test]
    fn summarize_tool_falls_back_to_name_only() {
        let input = serde_json::json!({"unknown_field": "value"});
        assert_eq!(summarize_tool("WeirdTool", &input), "WeirdTool");
    }

    #[test]
    fn summarize_tool_truncates_long_args() {
        let long_cmd = "a".repeat(120);
        let input = serde_json::json!({"command": long_cmd});
        let out = summarize_tool("Bash", &input);
        assert!(out.starts_with("Bash("));
        assert!(out.ends_with("...)"));
        assert!(out.len() < 80);
    }

    #[test]
    fn tool_summary_sorts_by_count_desc_then_name_asc() {
        let mut counts = HashMap::new();
        counts.insert("Read".to_string(), 3);
        counts.insert("Bash".to_string(), 1);
        counts.insert("Grep".to_string(), 1);
        counts.insert("Edit".to_string(), 2);
        assert_eq!(
            format_tool_summary(&counts),
            "Read x3, Edit x2, Bash x1, Grep x1"
        );
    }

    #[test]
    fn tool_summary_single_entry() {
        let mut counts = HashMap::new();
        counts.insert("Read".to_string(), 1);
        assert_eq!(format_tool_summary(&counts), "Read x1");
    }

    #[test]
    fn tool_summary_empty_returns_empty_string() {
        let counts: HashMap<String, usize> = HashMap::new();
        assert_eq!(format_tool_summary(&counts), "");
    }

    // -- format_permissions ------------------------------------------------

    fn bare_args() -> AskArgs {
        use clap::Parser;
        crate::cli::Cli::try_parse_from(["roba", "placeholder"])
            .unwrap()
            .ask
    }

    #[test]
    fn permissions_default_lists_safe_trio_as_default() {
        let out = format_permissions(&bare_args());
        assert_eq!(
            out,
            "allow:\n  Read [default]\n  Glob [default]\n  Grep [default]"
        );
    }

    #[test]
    fn permissions_writable_from_cli_tags_edit_write() {
        let mut args = bare_args();
        args.writable = true;
        args.writable_source = Some("CLI".to_string());
        let out = format_permissions(&args);
        assert!(out.contains("Edit  [CLI]"), "got:\n{out}");
        assert!(out.contains("Write [CLI]"), "got:\n{out}");
        // Safe trio still present and still tagged default.
        assert!(out.contains("Read  [default]"), "got:\n{out}");
    }

    #[test]
    fn permissions_writable_from_profile_layer() {
        let mut args = bare_args();
        args.writable = true;
        args.writable_source = Some("profile.review".to_string());
        let out = format_permissions(&args);
        assert!(out.contains("Edit  [profile.review]"), "got:\n{out}");
        assert!(out.contains("Write [profile.review]"), "got:\n{out}");
    }

    #[test]
    fn permissions_writable_from_env_layer() {
        let mut args = bare_args();
        args.writable = true;
        args.writable_source = Some("env".to_string());
        let out = format_permissions(&args);
        assert!(out.contains("Edit  [env]"), "got:\n{out}");
    }

    #[test]
    fn permissions_full_auto_is_single_line() {
        let mut args = bare_args();
        args.full_auto = true;
        args.full_auto_source = Some("profile.yolo".to_string());
        assert_eq!(
            format_permissions(&args),
            "all tools allowed (--full-auto from profile.yolo)"
        );
    }

    #[test]
    fn permissions_full_auto_defaults_source_to_cli() {
        let mut args = bare_args();
        args.full_auto = true;
        assert_eq!(
            format_permissions(&args),
            "all tools allowed (--full-auto from CLI)"
        );
    }

    #[test]
    fn permissions_includes_allow_and_deny_with_provenance() {
        let mut args = bare_args();
        args.allow_tool = vec!["Bash(git status)".to_string()];
        args.allow_tool_sources = vec!["profile.review".to_string()];
        args.deny_tool = vec!["Bash(rm *)".to_string()];
        args.deny_tool_sources = vec!["profile.review".to_string()];
        let out = format_permissions(&args);
        assert!(out.contains("allow:"), "got:\n{out}");
        assert!(
            out.contains("Bash(git status) [profile.review]"),
            "got:\n{out}"
        );
        assert!(out.contains("deny:"), "got:\n{out}");
        assert!(
            out.contains("Bash(rm *)       [profile.review]"),
            "got:\n{out}"
        );
    }

    #[test]
    fn permissions_columns_align_across_allow_and_deny() {
        // The deny entry is the longest name; allow rows pad to match.
        let mut args = bare_args();
        args.writable = true;
        args.writable_source = Some("profile.review".to_string());
        args.deny_tool = vec!["Bash(rm *)".to_string()];
        args.deny_tool_sources = vec!["profile.review".to_string()];
        let out = format_permissions(&args);
        // "Bash(rm *)" is 10 chars -> column width 11. "Read" + 7 spaces.
        assert!(out.contains("  Read       [default]"), "got:\n{out}");
        assert!(out.contains("  Bash(rm *) [profile.review]"), "got:\n{out}");
    }

    #[test]
    fn permissions_no_deny_section_when_empty() {
        let out = format_permissions(&bare_args());
        assert!(!out.contains("deny:"), "got:\n{out}");
    }
}
