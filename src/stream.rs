//! Streaming-mode pipeline (--stream).
//!
//! Drives `stream_query` from claude-wrapper. As assistant events
//! arrive, text content blocks flush to stdout and tool_use blocks
//! emit a one-line indicator on stderr (when metadata is enabled).
//! Tool calls are tallied for the rollup line at the end.

use anyhow::Result;
use claude_wrapper::streaming::stream_query;
use claude_wrapper::types::{OutputFormat, QueryResult};
use claude_wrapper::{Claude, QueryCommand};
use std::collections::HashMap;
use std::io::Write;

use crate::cli::AskArgs;
use crate::output::{format_footer, looks_like_refusal, should_show_footer, truncate_arg};
use crate::render::Style;
use crate::session::{apply_session, derive_session_name};

/// Run a prompt through the streaming pipeline. Text flushes to
/// stdout as it arrives; tool calls + cost footer + refusal warning
/// + tool rollup go to stderr at the appropriate moments.
pub async fn run_streaming(claude: &Claude, prompt: String, args: &AskArgs) -> Result<()> {
    let name = derive_session_name(&prompt);
    let cmd = apply_session(QueryCommand::new(prompt).name(name), args)
        .output_format(OutputFormat::StreamJson);
    let show_meta = should_show_footer(args);
    let style = Style::detect(args);
    let mut final_result: Option<QueryResult> = None;
    let mut tool_counts: HashMap<String, usize> = HashMap::new();

    stream_query(claude, &cmd, |event| {
        if event.is_result() {
            if let Ok(qr) = serde_json::from_value::<QueryResult>(event.data.clone()) {
                final_result = Some(qr);
            }
            return;
        }
        if event.event_type() == Some("assistant") {
            handle_assistant_blocks(&event.data, show_meta, &style, &mut tool_counts);
        }
    })
    .await?;
    println!();

    if show_meta && let Some(qr) = &final_result {
        crate::render::print_meta_blank();
        if looks_like_refusal(&qr.result) {
            crate::render::print_warning("response looks like a refusal", &style);
        }
        if !tool_counts.is_empty() {
            crate::render::print_meta(
                &format!("used: {}", format_tool_summary(&tool_counts)),
                &style,
            );
        }
        crate::render::print_meta(&format_footer(qr), &style);
    }
    Ok(())
}

/// Walk one assistant event's content blocks. Text blocks flush
/// directly to stdout; tool_use blocks tally into `tool_counts` and
/// optionally print an inline indicator on stderr.
pub fn handle_assistant_blocks(
    data: &serde_json::Value,
    show_meta: bool,
    style: &Style,
    tool_counts: &mut HashMap<String, usize>,
) {
    let Some(blocks) = data
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return;
    };
    for block in blocks {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    // Stream-friendly format: each line indented to
                    // match the non-stream body. termimad's full
                    // markdown render needs the whole text at once,
                    // so we don't get bold/headings/bullets here --
                    // but the indent + clean line breaks are cheap
                    // and keep the visual rhythm consistent with
                    // tool-call indent.
                    let trimmed = text.trim_end_matches(['\n', ' ', '\t']);
                    for line in trimmed.split('\n') {
                        if line.is_empty() {
                            println!();
                        } else {
                            println!("   {line}");
                        }
                    }
                    let _ = std::io::stdout().flush();
                }
            }
            Some("tool_use") => {
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                *tool_counts.entry(name.clone()).or_insert(0) += 1;
                if show_meta {
                    let input = block.get("input").unwrap_or(&serde_json::Value::Null);
                    crate::render::print_tool_call(&summarize_tool(&name, input), style);
                }
            }
            _ => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn handle_assistant_blocks_counts_tool_uses() {
        let event = serde_json::json!({
            "message": {
                "content": [
                    {"type": "tool_use", "name": "Read", "input": {"file_path": "a"}},
                    {"type": "tool_use", "name": "Read", "input": {"file_path": "b"}},
                    {"type": "tool_use", "name": "Bash", "input": {"command": "ls"}},
                ]
            }
        });
        let mut counts = HashMap::new();
        handle_assistant_blocks(&event, false, &Style::plain(), &mut counts);
        assert_eq!(counts.get("Read"), Some(&2));
        assert_eq!(counts.get("Bash"), Some(&1));
    }

    #[test]
    fn handle_assistant_blocks_handles_missing_content() {
        let event = serde_json::json!({"message": {}});
        let mut counts = HashMap::new();
        handle_assistant_blocks(&event, false, &Style::plain(), &mut counts);
        assert!(counts.is_empty());
    }

    #[test]
    fn handle_assistant_blocks_handles_missing_message() {
        let event = serde_json::json!({});
        let mut counts = HashMap::new();
        handle_assistant_blocks(&event, false, &Style::plain(), &mut counts);
        assert!(counts.is_empty());
    }

    #[test]
    fn handle_assistant_blocks_ignores_unknown_block_types() {
        let event = serde_json::json!({
            "message": {
                "content": [
                    {"type": "future_kind", "data": "whatever"},
                    {"type": "tool_use", "name": "Read", "input": {}},
                ]
            }
        });
        let mut counts = HashMap::new();
        handle_assistant_blocks(&event, false, &Style::plain(), &mut counts);
        assert_eq!(counts.get("Read"), Some(&1));
        assert_eq!(counts.len(), 1);
    }

    #[test]
    fn handle_assistant_blocks_uses_question_mark_for_missing_name() {
        let event = serde_json::json!({
            "message": {
                "content": [{"type": "tool_use", "input": {}}]
            }
        });
        let mut counts = HashMap::new();
        handle_assistant_blocks(&event, false, &Style::plain(), &mut counts);
        assert_eq!(counts.get("?"), Some(&1));
    }
}
