use anyhow::{Context, Result, bail};
use clap::Parser;
use claude_wrapper::types::QueryResult;
use claude_wrapper::{Claude, QueryCommand};
use std::io::{IsTerminal, Read};
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
        eprintln!("{}", format_footer(&result));
    }
    Ok(())
}

fn path_is_json(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("json")
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
}

fn should_show_footer(args: &Args) -> bool {
    !args.quiet && std::io::stderr().is_terminal()
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
