//! Streaming-mode pipeline (--stream).
//!
//! Drives `stream_query` from claude-wrapper. As assistant events
//! arrive, text content blocks flush to stdout and tool_use blocks
//! emit a one-line indicator on stderr (when metadata is enabled).
//! Tool calls are tallied for the rollup line at the end.

use anyhow::Result;
use claude_wrapper::streaming::{BlockDelta, PartialMessageEvent, stream_query};
use claude_wrapper::types::{OutputFormat, QueryResult};
use claude_wrapper::{Claude, QueryCommand};
use std::collections::HashMap;
use std::io::Write;

use crate::cli::AskArgs;
use crate::output::{
    format_footer, format_tool_summary, looks_like_refusal, should_show_footer, summarize_tool,
};
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
        if args.show_thinking
            && let Some(PartialMessageEvent::BlockDelta {
                delta: BlockDelta::Thinking(text),
                ..
            }) = event.partial_message()
        {
            render_thinking_delta(&text, &style);
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

/// Render a thinking-block delta to stderr in the dim meta style.
/// Chunks arrive piecewise so this hands off to
/// [`crate::render::print_thinking_delta`], which writes without a
/// trailing newline.
fn render_thinking_delta(text: &str, style: &Style) {
    crate::render::print_thinking_delta(text, style);
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

#[cfg(test)]
mod tests {
    use super::*;

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
