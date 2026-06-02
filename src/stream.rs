//! Streaming-mode pipeline (--stream).
//!
//! Drives `stream_query` from claude-wrapper. As assistant events
//! arrive, text content blocks flush to stdout and tool_use blocks
//! emit a one-line indicator on stderr (when metadata is enabled).
//! Tool calls are tallied for the rollup line at the end.

use anyhow::{Context, Result};
use claude_wrapper::streaming::{BlockDelta, PartialMessageEvent, StreamEvent, stream_query};
use claude_wrapper::types::{OutputFormat, QueryResult};
use claude_wrapper::{Claude, QueryCommand};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::cli::AskArgs;
use crate::output::{
    format_footer, format_tool_summary, looks_like_refusal, should_show_footer, summarize_tool,
};
use crate::render::Style;
use crate::session::{apply_session, derive_session_name};

/// How a streaming run presents itself while events arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// Live token + tool-call streaming to stdout/stderr, plus the
    /// cost footer at the end. This is the `--stream` experience.
    Live,
    /// No live display at all -- only the trace (if any) is written and
    /// the final [`QueryResult`] is collected for the caller to render
    /// the way the non-streaming path would. Used when `--trace` is on
    /// but the user did not ask for `--stream`.
    Silent,
}

/// Buffered JSONL trace sink. Each stream event is serialized back to
/// its raw wire JSON and written as one line, in arrival order. The
/// trace mirrors the spawned session's NDJSON, not roba's typed view.
struct TraceWriter {
    out: BufWriter<File>,
}

impl TraceWriter {
    fn write_event(&mut self, event: &StreamEvent) -> Result<()> {
        serde_json::to_writer(&mut self.out, event).context("serializing stream event")?;
        self.out.write_all(b"\n").context("writing trace newline")?;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.out.flush().context("flushing trace file")
    }
}

/// Open a trace sink when a path is set. Parent directories are
/// created if missing; an existing file is truncated. Returns `None`
/// when no `--trace` path was requested.
fn open_trace(path: Option<&Path>) -> Result<Option<TraceWriter>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating trace parent dir {}", parent.display()))?;
    }
    let file =
        File::create(path).with_context(|| format!("creating trace file {}", path.display()))?;
    Ok(Some(TraceWriter {
        out: BufWriter::new(file),
    }))
}

/// Run a prompt through the streaming pipeline.
///
/// When `args.trace` is set, every event is also written to the trace
/// file as a JSON line, in arrival order, regardless of `display`.
///
/// In [`DisplayMode::Live`] text flushes to stdout as it arrives, tool
/// calls + cost footer + refusal warning + tool rollup go to stderr at
/// the appropriate moments, and the call returns `Ok(None)` (it
/// rendered everything itself). In [`DisplayMode::Silent`] nothing is
/// displayed and the final [`QueryResult`] is returned for the caller
/// to render exactly as the non-streaming path would.
pub async fn run_streaming(
    claude: &Claude,
    prompt: String,
    args: &AskArgs,
    display: DisplayMode,
) -> Result<Option<QueryResult>> {
    let name = derive_session_name(&prompt);
    let cmd = apply_session(QueryCommand::new(prompt).name(name), args)
        .output_format(OutputFormat::StreamJson);
    let show_meta = should_show_footer(args);
    let style = Style::detect(args);
    let mut final_result: Option<QueryResult> = None;
    let mut tool_counts: HashMap<String, usize> = HashMap::new();
    let mut trace = open_trace(args.trace.as_deref())?;
    let mut trace_err: Option<anyhow::Error> = None;

    let mut session_id_printed = false;

    let stream_result = stream_query(claude, &cmd, |event| {
        if let Some(t) = trace.as_mut()
            && let Err(e) = t.write_event(&event)
            && trace_err.is_none()
        {
            trace_err = Some(e);
        }
        // Print the spawned session id to stderr on the first event that
        // carries it. One line per dispatch so orchestrators and humans can
        // find the session JSONL without needing --trace. Not gated by TTY
        // (orchestrators running without a TTY still need it); gated by
        // --quiet because it is metadata.
        if !session_id_printed
            && !args.quiet
            && let Some(id) = event.session_id()
        {
            eprintln!("[roba] session: {id}");
            session_id_printed = true;
        }
        if event.is_result() {
            if let Ok(qr) = serde_json::from_value::<QueryResult>(event.data.clone()) {
                final_result = Some(qr);
            }
            return;
        }
        if display == DisplayMode::Silent {
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
    .await;

    // Flush whatever events made it into the trace before surfacing any
    // error -- a partial trace is the whole point of an observability
    // handle when a run dies mid-flight.
    if let Some(t) = trace.as_mut() {
        let _ = t.flush();
    }
    stream_result?;
    if let Some(e) = trace_err {
        return Err(e);
    }

    if display == DisplayMode::Silent {
        return Ok(final_result);
    }

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
        let rates = if args.no_dollars {
            None
        } else {
            crate::rates::Rates::resolve(args.rates_file.as_deref()).ok()
        };
        crate::render::print_meta(&format_footer(qr, rates.as_ref(), args.no_dollars), &style);
    }
    Ok(None)
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
    fn open_trace_none_path_is_none() {
        assert!(open_trace(None).unwrap().is_none());
    }

    #[test]
    fn open_trace_creates_parent_dirs_and_writes_jsonl() {
        let dir = std::env::temp_dir().join(format!("roba-trace-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("run.jsonl");

        let mut tw = open_trace(Some(&path)).unwrap().expect("trace writer");
        let ev: StreamEvent =
            serde_json::from_value(serde_json::json!({"type": "assistant", "n": 1})).unwrap();
        tw.write_event(&ev).unwrap();
        let ev2: StreamEvent =
            serde_json::from_value(serde_json::json!({"type": "result", "n": 2})).unwrap();
        tw.write_event(&ev2).unwrap();
        tw.flush().unwrap();
        drop(tw);

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "one JSON line per event");
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["type"], "assistant");
        assert_eq!(first["n"], 1);
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["type"], "result");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_trace_truncates_existing_file() {
        let path =
            std::env::temp_dir().join(format!("roba-trace-trunc-{}.jsonl", std::process::id()));
        std::fs::write(&path, "stale stale stale\nmore stale\n").unwrap();

        let mut tw = open_trace(Some(&path)).unwrap().expect("trace writer");
        let ev: StreamEvent =
            serde_json::from_value(serde_json::json!({"type": "system"})).unwrap();
        tw.write_event(&ev).unwrap();
        tw.flush().unwrap();
        drop(tw);

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            !body.contains("stale"),
            "existing content must be truncated"
        );
        assert_eq!(body.lines().count(), 1);

        let _ = std::fs::remove_file(&path);
    }

    // The `session_id_printed` guard in `run_streaming` uses
    // `StreamEvent::session_id()`. Verify the accessor returns the field
    // from the event data so the guard behaves as documented.
    #[test]
    fn stream_event_session_id_accessor() {
        let ev: StreamEvent = serde_json::from_value(serde_json::json!({
            "type": "system",
            "session_id": "abc-123"
        }))
        .unwrap();
        assert_eq!(ev.session_id(), Some("abc-123"));

        let no_id: StreamEvent = serde_json::from_value(serde_json::json!({
            "type": "system"
        }))
        .unwrap();
        assert_eq!(no_id.session_id(), None);
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
