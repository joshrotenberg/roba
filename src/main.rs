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
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}", result.result);
        if should_show_footer(&args) {
            eprintln!();
            eprintln!("{}", format_footer(&result));
        }
    }
    Ok(())
}

fn should_show_footer(args: &Args) -> bool {
    !args.quiet && !args.json && std::io::stderr().is_terminal()
}

fn format_footer(r: &QueryResult) -> String {
    let mut parts = Vec::new();
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
