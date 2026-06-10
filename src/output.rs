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

/// Body for the default text path (not `--json`, not `--code`): the
/// textual `result.result` when it has content, otherwise the validated
/// `structured_output` rendered as pretty JSON.
///
/// `--json-schema` lands the answer in `structured_output` (which
/// flattens into `result.extra`) and leaves the textual `result` empty.
/// Without this fallback the default path prints a blank line and exits
/// 0 -- the answer was computed and paid for but never shown. The pretty
/// JSON stays pipeable: `roba --json-schema s.json "..." | jq` sees the
/// object. Returns empty only when both are absent.
pub fn default_body(result: &QueryResult) -> String {
    if !result.result.is_empty() {
        return result.result.clone();
    }
    match result.extra.get("structured_output") {
        Some(value) if !value.is_null() => serde_json::to_string_pretty(value).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Single-line footer summarizing a [`QueryResult`]: tokens, cost,
/// duration, model (+ effort), session id (first 8 chars).
///
/// The dollar figure prefers claude's own authoritative `total_cost_usd`
/// when present; otherwise, if `rates` is supplied and the call's model
/// is in the table, it is computed from the rate table. `no_dollars`
/// suppresses the figure entirely.
///
/// The model/effort segment slots between duration and session.
/// `model_override` is the resolved `--model` (after env/profile merge),
/// used as a fallback when claude's `modelUsage` is absent; `effort` is
/// the resolved client-side effort label, appended only when a model is
/// shown. See `format_model_segment`.
pub fn format_footer(
    r: &QueryResult,
    rates: Option<&crate::rates::Rates>,
    no_dollars: bool,
    model_override: Option<&str>,
    effort: Option<&str>,
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
    if let Some(seg) = format_model_segment(&r.extra, model_override, effort) {
        parts.push(seg);
    }
    let id = r.session_id.get(..8).unwrap_or(&r.session_id);
    parts.push(format!("session {id}"));
    parts.join(" . ")
}

/// The `model[+N][/effort]` footer segment, or `None` when no model is
/// known (in which case effort is dropped too -- a bare `/high` is noise).
///
/// Model comes from the result's `modelUsage` map (the authoritative
/// per-run record claude emits, keyed by real model ids). One key -> that
/// model; multiple keys (a `--fallback-model` fired) -> the model with the
/// largest output-token usage plus `+N` counting the rest. When
/// `modelUsage` is absent or empty, falls back to `model_override` (the
/// resolved `--model`). The `claude-` prefix is stripped for density.
/// `effort` is client-side only and appended as `/effort` only when a
/// model is shown.
fn format_model_segment(
    extra: &HashMap<String, serde_json::Value>,
    model_override: Option<&str>,
    effort: Option<&str>,
) -> Option<String> {
    let mut seg = model_segment_base(extra, model_override)?;
    if let Some(e) = effort {
        seg.push('/');
        seg.push_str(e);
    }
    Some(seg)
}

/// The bare model portion of the footer segment (no effort suffix).
fn model_segment_base(
    extra: &HashMap<String, serde_json::Value>,
    model_override: Option<&str>,
) -> Option<String> {
    if let Some(map) = extra.get("modelUsage").and_then(|v| v.as_object())
        && !map.is_empty()
    {
        let output = |v: &serde_json::Value| v.get("outputTokens").and_then(|x| x.as_u64());
        let primary = map.iter().max_by_key(|(_, v)| output(v).unwrap_or(0))?.0;
        let mut s = strip_claude_prefix(primary).to_string();
        let extras = map.len() - 1;
        if extras > 0 {
            s.push_str(&format!("+{extras}"));
        }
        return Some(s);
    }
    model_override.map(|m| strip_claude_prefix(m).to_string())
}

/// Strip a leading `claude-` for display density (`claude-opus-4-8` ->
/// `opus-4-8`); pass anything else through unchanged.
fn strip_claude_prefix(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
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
///
/// The thresholds are rounding-aware so a value never displays as
/// `1000.0k`: anything that would round up to `1000.0k` at one decimal
/// (i.e. `>= 999_950`) promotes to the `M` form instead. `M` is the top
/// unit, so very large counts keep growing as `M` (e.g. `1000.0M`).
pub fn format_count(n: u64) -> String {
    if n >= 999_950 {
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
    fn default_body_prefers_textual_result() {
        // A normal answer: textual result present, structured_output
        // ignored even when both are set.
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "the answer is 42",
            "session_id": "s1",
            "is_error": false,
            "structured_output": {"answer": "42"},
        }))
        .expect("build QueryResult fixture");
        assert_eq!(default_body(&result), "the answer is 42");
    }

    #[test]
    fn default_body_renders_structured_output_when_result_empty() {
        // The --json-schema default path: empty textual result, answer
        // in structured_output -> render it as pretty JSON.
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "",
            "session_id": "s1",
            "is_error": false,
            "structured_output": {"answer": "Paris"},
        }))
        .expect("build QueryResult fixture");
        let body = default_body(&result);
        let value: serde_json::Value = serde_json::from_str(&body).expect("body parses as JSON");
        assert_eq!(value["answer"], "Paris");
        assert!(body.contains('\n'), "pretty-printed JSON is multi-line");
    }

    #[test]
    fn default_body_empty_when_both_absent() {
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "",
            "session_id": "s1",
            "is_error": false,
        }))
        .expect("build QueryResult fixture");
        assert_eq!(default_body(&result), "");
    }

    #[test]
    fn default_body_empty_when_structured_output_is_null() {
        // A `null` structured_output is treated as absent, not rendered
        // as the literal string "null".
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "",
            "session_id": "s1",
            "is_error": false,
            "structured_output": null,
        }))
        .expect("build QueryResult fixture");
        assert_eq!(default_body(&result), "");
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
        // Just below the rounding-promotion boundary: still the k form.
        assert_eq!(format_count(999_949), "999.9k");
    }

    #[test]
    fn format_count_million_scale() {
        assert_eq!(format_count(1_000_000), "1.0M");
        assert_eq!(format_count(1_500_000), "1.5M");
    }

    #[test]
    fn format_count_k_to_m_rounding_boundary() {
        // A value that would round to "1000.0k" promotes to "1.0M"
        // instead (the #273 fix); the exact boundary is 999_950.
        assert_eq!(format_count(999_950), "1.0M");
        assert_eq!(format_count(999_999), "1.0M");
        assert_eq!(format_count(999_949), "999.9k");
        // The plain/k boundary is unchanged: 999 stays plain, 1_000 is k.
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1.0k");
        // M is the top unit, so very large counts keep growing as M.
        assert_eq!(format_count(999_999_999), "1000.0M");
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
        let footer = format_footer(&result, None, false, None, None);
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
        let footer = format_footer(&result, None, true, None, None);
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
        let footer = format_footer(&result, Some(&rates), false, None, None);
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
        let footer = format_footer(&result, Some(&rates), false, None, None);
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
        let footer = format_footer(&result, None, false, None, None);
        assert!(!footer.contains("tokens"));
        assert!(!footer.contains('$'));
        assert!(footer.contains("session deadbeef"));
    }

    /// Build a minimal result carrying just a `modelUsage` map.
    fn result_with_model_usage(usage: serde_json::Value) -> QueryResult {
        serde_json::from_value(serde_json::json!({
            "result": "ok",
            "session_id": "deadbeef",
            "is_error": false,
            "modelUsage": usage,
        }))
        .expect("build QueryResult fixture")
    }

    #[test]
    fn footer_segment_single_model_strips_claude_prefix() {
        let result = result_with_model_usage(serde_json::json!({
            "claude-opus-4-8": {"outputTokens": 100},
        }));
        let footer = format_footer(&result, None, false, None, None);
        assert_eq!(footer, "opus-4-8 . session deadbeef", "got: {footer}");
    }

    #[test]
    fn footer_segment_multi_model_picks_largest_output_plus_n() {
        // sonnet has the larger output usage -> it is primary, +1 for opus.
        let result = result_with_model_usage(serde_json::json!({
            "claude-opus-4-8": {"outputTokens": 10},
            "claude-sonnet-4-6": {"outputTokens": 900},
        }));
        let footer = format_footer(&result, None, false, None, None);
        assert!(footer.contains("sonnet-4-6+1"), "got: {footer}");
    }

    #[test]
    fn footer_segment_appends_effort_when_set() {
        let result = result_with_model_usage(serde_json::json!({
            "claude-haiku-4-5": {"outputTokens": 5},
        }));
        let footer = format_footer(&result, None, false, None, Some("high"));
        assert!(footer.contains("haiku-4-5/high"), "got: {footer}");
    }

    #[test]
    fn footer_segment_no_effort_suffix_when_unset() {
        let result = result_with_model_usage(serde_json::json!({
            "claude-haiku-4-5": {"outputTokens": 5},
        }));
        let footer = format_footer(&result, None, false, None, None);
        assert!(footer.contains("haiku-4-5"), "got: {footer}");
        assert!(!footer.contains('/'), "got: {footer}");
    }

    #[test]
    fn footer_segment_falls_back_to_model_override() {
        // No modelUsage in the result; the resolved --model fills in.
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "ok",
            "session_id": "deadbeef",
            "is_error": false,
        }))
        .expect("build QueryResult fixture");
        let footer = format_footer(&result, None, false, Some("claude-opus-4-8"), None);
        assert!(footer.contains("opus-4-8"), "got: {footer}");
    }

    #[test]
    fn footer_segment_omitted_when_no_model_anywhere() {
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "ok",
            "session_id": "deadbeef",
            "is_error": false,
        }))
        .expect("build QueryResult fixture");
        // Effort set but no model -> the whole segment (and effort) is dropped.
        let footer = format_footer(&result, None, false, None, Some("high"));
        assert_eq!(footer, "session deadbeef", "got: {footer}");
    }

    #[test]
    fn footer_segment_empty_model_usage_falls_back_to_override() {
        let result = result_with_model_usage(serde_json::json!({}));
        let footer = format_footer(&result, None, false, Some("claude-haiku-4-5"), None);
        assert!(footer.contains("haiku-4-5"), "got: {footer}");
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
