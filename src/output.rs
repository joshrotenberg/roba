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

/// `.json` extension check for --save / --tee format inference.
pub fn path_is_json(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("json")
}

/// Keep only the first N (`head`) or last N (`tail`) lines of `text`.
/// `head` wins if both are set (callers should enforce mutual exclusion).
pub fn truncate_lines(text: &str, head: Option<usize>, tail: Option<usize>) -> String {
    match (head, tail) {
        (Some(n), _) => text.lines().take(n).collect::<Vec<_>>().join("\n"),
        (_, Some(n)) => {
            let all: Vec<&str> = text.lines().collect();
            let start = all.len().saturating_sub(n);
            all[start..].join("\n")
        }
        _ => text.to_string(),
    }
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
pub fn format_footer(r: &QueryResult) -> String {
    let mut parts = Vec::new();
    if let Some((input, output)) = extract_tokens(&r.extra) {
        parts.push(format!(
            "tokens {}/{}",
            format_count(input),
            format_count(output)
        ));
    }
    if let Some(cost) = r.cost_usd {
        parts.push(format!("cost ${cost:.4}"));
    }
    if let Some(ms) = r.duration_ms {
        parts.push(format_duration(ms));
    }
    let id = r.session_id.get(..8).unwrap_or(&r.session_id);
    parts.push(format!("session {id}"));
    parts.join(" . ")
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
    fn truncate_lines_head_keeps_first_n() {
        let body = "a\nb\nc\nd\ne";
        assert_eq!(truncate_lines(body, Some(3), None), "a\nb\nc");
    }

    #[test]
    fn truncate_lines_tail_keeps_last_n() {
        let body = "a\nb\nc\nd\ne";
        assert_eq!(truncate_lines(body, None, Some(2)), "d\ne");
    }

    #[test]
    fn truncate_lines_head_larger_than_input_keeps_all() {
        let body = "a\nb";
        assert_eq!(truncate_lines(body, Some(99), None), "a\nb");
    }

    #[test]
    fn truncate_lines_tail_larger_than_input_keeps_all() {
        let body = "a\nb";
        assert_eq!(truncate_lines(body, None, Some(99)), "a\nb");
    }

    #[test]
    fn truncate_lines_no_op_when_both_none() {
        let body = "a\nb\nc";
        assert_eq!(truncate_lines(body, None, None), "a\nb\nc");
    }

    #[test]
    fn truncate_lines_zero_returns_empty() {
        let body = "a\nb\nc";
        assert_eq!(truncate_lines(body, Some(0), None), "");
        assert_eq!(truncate_lines(body, None, Some(0)), "");
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
        let footer = format_footer(&result);
        assert!(footer.contains("tokens 6/14"));
        assert!(footer.contains("cost $0.0192"));
        assert!(footer.contains("1.8s"));
        assert!(footer.contains("session abc12345"));
    }

    #[test]
    fn format_footer_drops_missing_segments() {
        let result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "ok",
            "session_id": "deadbeef",
            "is_error": false,
        }))
        .expect("build QueryResult fixture");
        let footer = format_footer(&result);
        assert!(!footer.contains("tokens"));
        assert!(!footer.contains("cost"));
        assert!(footer.contains("session deadbeef"));
    }
}
