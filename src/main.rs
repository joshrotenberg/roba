use anyhow::{Context, Result, bail};
use clap::Parser;
use claude_wrapper::{Claude, ClaudeCommand, QueryCommand};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;

/// Single-prompt CLI runner built on claude-wrapper.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The prompt to send to Claude. Pass `-` to read from stdin
    /// explicitly. If omitted and stdin is piped, stdin is used.
    #[arg(conflicts_with = "file")]
    prompt: Option<String>,

    /// Read the prompt from a file.
    #[arg(short, long, value_name = "PATH")]
    file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let prompt = resolve_prompt(args.prompt, args.file)?;
    let claude = Claude::builder().build()?;
    let output = QueryCommand::new(prompt).execute(&claude).await?;
    print!("{}", output.stdout);
    Ok(())
}

fn resolve_prompt(positional: Option<String>, file: Option<PathBuf>) -> Result<String> {
    if let Some(path) = file {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading prompt from {}", path.display()))?;
        let trimmed = content.trim_end().to_string();
        if trimmed.is_empty() {
            bail!("file {} is empty", path.display());
        }
        return Ok(trimmed);
    }
    match positional.as_deref() {
        Some("-") => read_stdin(),
        Some(p) => Ok(p.to_string()),
        None => {
            if std::io::stdin().is_terminal() {
                bail!(
                    "no prompt: pass one as an argument, use -f <path>, pipe via stdin, or use `-` to read stdin explicitly"
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
