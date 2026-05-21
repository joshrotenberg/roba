use anyhow::{Context, Result, bail};
use clap::Parser;
use claude_wrapper::streaming::stream_query;
use claude_wrapper::types::{OutputFormat, QueryResult};
use claude_wrapper::{Claude, QueryCommand};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Single-prompt CLI runner built on claude-wrapper.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The prompt to send to Claude. Pass `-` to read from stdin
    /// explicitly. If omitted and stdin is piped, stdin is used.
    #[arg(conflicts_with_all = ["file", "editor"])]
    prompt: Option<String>,

    /// Read the prompt from a file.
    #[arg(short, long, value_name = "PATH", conflicts_with = "editor")]
    file: Option<PathBuf>,

    /// Open $VISUAL or $EDITOR (falling back to vi) to compose the
    /// prompt. The editor opens an empty markdown buffer; on save +
    /// exit the content becomes the prompt. Aborts on non-zero
    /// editor exit or empty buffer.
    #[arg(short = 'e', long = "editor")]
    editor: bool,

    /// Print the resolved prompt before the response, separated by
    /// a divider. Useful with -e / -f / stdin where the sent prompt
    /// isn't already on screen.
    #[arg(long)]
    echo: bool,

    /// Suppress everything except the answer on stdout. Overrides
    /// --echo and (later) the cost footer / any other stderr noise.
    /// Use when you want the cleanest possible output even on a TTY.
    #[arg(short = 'q', long)]
    quiet: bool,

    /// Emit the full structured result as JSON on stdout instead of
    /// the plain answer. Includes session_id, cost_usd, duration_ms,
    /// num_turns, is_error, and the answer text. Pretty-printed.
    #[arg(long)]
    json: bool,

    /// Write the result to PATH instead of stdout. If the path ends
    /// in .json, the full structured record is written; otherwise
    /// the plain answer. --json overrides the extension and forces
    /// JSON regardless.
    #[arg(long, value_name = "PATH")]
    save: Option<PathBuf>,

    /// Write the result to both stdout and PATH (like Unix tee).
    /// Same extension-driven format rules as --save.
    #[arg(long, value_name = "PATH", conflicts_with = "save")]
    tee: Option<PathBuf>,

    /// Print only fenced code blocks from the answer. Pass with no
    /// value to take every block; pass --code rust to filter by
    /// language. Multiple blocks are separated by a blank line.
    /// Mutually exclusive with --json.
    #[arg(
        long,
        value_name = "LANG",
        num_args(0..=1),
        default_missing_value = "",
        conflicts_with = "json",
    )]
    code: Option<String>,

    /// Truncate output to the first N lines. Applied after --code.
    /// Mutually exclusive with --tail and --json.
    #[arg(long, value_name = "N", conflicts_with_all = ["tail", "json"])]
    head: Option<usize>,

    /// Truncate output to the last N lines. Applied after --code.
    /// Mutually exclusive with --head and --json.
    #[arg(long, value_name = "N", conflicts_with_all = ["head", "json"])]
    tail: Option<usize>,

    /// Stream the response to stdout as it arrives instead of waiting
    /// for the full result. Mutually exclusive with the output-shaping
    /// flags (--json, --code, --head, --tail, --save, --tee) since
    /// they all need the final body assembled before they can act.
    #[arg(
        long,
        conflicts_with_all = ["json", "code", "head", "tail", "save", "tee"],
    )]
    stream: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let prompt = resolve_prompt(args.prompt.as_deref(), args.file.as_deref(), args.editor)?;
    if args.echo && !args.quiet {
        eprintln!("{prompt}");
        eprintln!();
        eprintln!("---");
        eprintln!();
    }
    let claude = Claude::builder().build()?;

    if args.stream {
        return run_streaming(&claude, prompt, &args).await;
    }

    let result = QueryCommand::new(prompt).execute_json(&claude).await?;

    let file_path = args.tee.as_deref().or(args.save.as_deref());
    let want_json = args.json || file_path.is_some_and(path_is_json);
    let body = if want_json {
        serde_json::to_string_pretty(&result)?
    } else if let Some(filter) = args.code.as_deref() {
        let lang = if filter.is_empty() { None } else { Some(filter) };
        extract_code_blocks(&result.result, lang)
    } else {
        result.result.clone()
    };
    let body = truncate_lines(&body, args.head, args.tail);

    let write_stdout = args.save.is_none();
    if write_stdout {
        println!("{body}");
    }
    if let Some(path) = file_path {
        std::fs::write(path, format!("{body}\n"))
            .with_context(|| format!("writing result to {}", path.display()))?;
    }
    if should_show_footer(&args) {
        eprintln!();
        if looks_like_refusal(&result.result) {
            eprintln!("warning: response looks like a refusal");
        }
        eprintln!("{}", format_footer(&result));
    }
    Ok(())
}

fn looks_like_refusal(text: &str) -> bool {
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

fn path_is_json(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("json")
}

fn truncate_lines(text: &str, head: Option<usize>, tail: Option<usize>) -> String {
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

fn extract_code_blocks(text: &str, lang_filter: Option<&str>) -> String {
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
        let mut extra = std::collections::HashMap::new();
        extra.insert(
            "usage".to_string(),
            serde_json::json!({"input_tokens": 42, "output_tokens": 7}),
        );
        assert_eq!(extract_tokens(&extra), Some((42, 7)));
    }

    #[test]
    fn extract_tokens_returns_none_when_usage_missing() {
        let extra = std::collections::HashMap::new();
        assert_eq!(extract_tokens(&extra), None);
    }

    #[test]
    fn extract_tokens_returns_none_on_wrong_shape() {
        let mut extra = std::collections::HashMap::new();
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
        // a refusal-y phrase deep in a long answer is probably the model
        // talking about refusals, not actually refusing
        assert!(!looks_like_refusal(
            "Yes, here's how. Note that I can't help with the part about X."
        ));
    }
}

fn should_show_footer(args: &Args) -> bool {
    !args.quiet && std::io::stderr().is_terminal()
}

async fn run_streaming(claude: &Claude, prompt: String, args: &Args) -> Result<()> {
    let cmd = QueryCommand::new(prompt).output_format(OutputFormat::StreamJson);
    let mut final_result: Option<QueryResult> = None;

    stream_query(claude, &cmd, |event| {
        if event.is_result() {
            if let Ok(qr) = serde_json::from_value::<QueryResult>(event.data.clone()) {
                final_result = Some(qr);
            }
            return;
        }
        if event.event_type() == Some("assistant")
            && let Some(text) = extract_assistant_text(&event.data)
        {
            print!("{text}");
            let _ = std::io::stdout().flush();
        }
    })
    .await?;
    println!();

    if should_show_footer(args)
        && let Some(qr) = &final_result
    {
        eprintln!();
        if looks_like_refusal(&qr.result) {
            eprintln!("warning: response looks like a refusal");
        }
        eprintln!("{}", format_footer(qr));
    }
    Ok(())
}

fn extract_assistant_text(data: &serde_json::Value) -> Option<String> {
    let content = data.get("message")?.get("content")?.as_array()?;
    let mut out = String::new();
    for block in content {
        if block.get("type").and_then(|t| t.as_str()) == Some("text")
            && let Some(text) = block.get("text").and_then(|v| v.as_str())
        {
            out.push_str(text);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn format_footer(r: &QueryResult) -> String {
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

fn extract_tokens(
    extra: &std::collections::HashMap<String, serde_json::Value>,
) -> Option<(u64, u64)> {
    let usage = extra.get("usage")?;
    let input = usage.get("input_tokens")?.as_u64()?;
    let output = usage.get("output_tokens")?.as_u64()?;
    Some((input, output))
}

fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}

fn format_duration(ms: u64) -> String {
    let secs = ms as f64 / 1000.0;
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let m = (secs / 60.0) as u64;
        let s = secs - (m as f64) * 60.0;
        format!("{m}m{s:.0}s")
    }
}

fn resolve_prompt(positional: Option<&str>, file: Option<&Path>, editor: bool) -> Result<String> {
    if editor {
        if !std::io::stdin().is_terminal() {
            bail!("--editor requires a TTY; pipe-mode input is incompatible");
        }
        return compose_in_editor();
    }
    if let Some(path) = file {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading prompt from {}", path.display()))?;
        let trimmed = content.trim_end().to_string();
        if trimmed.is_empty() {
            bail!("file {} is empty", path.display());
        }
        return Ok(trimmed);
    }
    match positional {
        Some("-") => read_stdin(),
        Some(p) => Ok(p.to_string()),
        None => {
            if std::io::stdin().is_terminal() {
                bail!(
                    "no prompt: pass one as an argument, use -f <path>, use -e for an editor, pipe via stdin, or use `-` to read stdin explicitly"
                );
            }
            read_stdin()
        }
    }
}

fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let trimmed = buf.trim_end().to_string();
    if trimmed.is_empty() {
        bail!("empty stdin");
    }
    Ok(trimmed)
}

fn compose_in_editor() -> Result<String> {
    let tmp = tempfile::Builder::new()
        .prefix("cwr-prompt-")
        .suffix(".md")
        .tempfile()
        .context("creating editor scratch file")?;
    let path = tmp.path().to_path_buf();
    let editor = editor_command();
    let status = spawn_editor(&editor, &path)
        .with_context(|| format!("running editor `{}`", editor))?;
    if !status.success() {
        bail!("editor exited with {status}");
    }
    let content = std::fs::read_to_string(&path).context("reading editor buffer")?;
    let trimmed = content.trim_end().to_string();
    if trimmed.is_empty() {
        bail!("editor returned an empty prompt");
    }
    Ok(trimmed)
}

fn editor_command() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string())
}

fn spawn_editor(editor: &str, path: &Path) -> std::io::Result<std::process::ExitStatus> {
    let mut parts = editor.split_whitespace();
    let program = parts.next().expect("editor_command never returns empty");
    let extra_args: Vec<&str> = parts.collect();
    Command::new(program).args(&extra_args).arg(path).status()
}
