use anyhow::Result;
use clap::Parser;
use claude_wrapper::{Claude, ClaudeCommand, QueryCommand};

/// Single-prompt CLI runner built on claude-wrapper.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The prompt to send to Claude.
    prompt: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let claude = Claude::builder().build()?;
    let output = QueryCommand::new(args.prompt).execute(&claude).await?;
    print!("{}", output.stdout);
    Ok(())
}
