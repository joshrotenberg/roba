use anyhow::{Context, Result, bail};
use clap::{Args as ClapArgs, Parser, Subcommand};
use claude_wrapper::streaming::stream_query;
use claude_wrapper::types::{OutputFormat, QueryResult};
use claude_wrapper::{Claude, QueryCommand};
use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Single-prompt CLI runner built on claude-wrapper.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<SubCommand>,

    #[command(flatten)]
    ask: AskArgs,
}

#[derive(Subcommand, Debug)]
enum SubCommand {
    /// List recent sessions across all projects.
    History(HistoryArgs),
    /// Reprint the most recent session's last answer.
    Last(LastArgs),
}

#[derive(ClapArgs, Debug)]
struct LastArgs {
    /// Filter to one project by slug. Project slugs start with `-`
    /// so this accepts hyphen-prefixed values without quoting.
    #[arg(long, value_name = "SLUG", allow_hyphen_values = true)]
    project: Option<String>,
}

#[derive(ClapArgs, Debug)]
struct HistoryArgs {
    /// Maximum number of sessions to show (default 10).
    #[arg(short = 'n', long, value_name = "N")]
    limit: Option<usize>,

    /// Show all sessions (no limit). Overrides --limit.
    #[arg(long, conflicts_with = "limit")]
    all: bool,

    /// Filter to one project by slug. Project slugs start with `-`
    /// so this accepts hyphen-prefixed values without quoting.
    #[arg(long, value_name = "SLUG", allow_hyphen_values = true)]
    project: Option<String>,

    /// Emit JSON instead of a human table.
    #[arg(long)]
    json: bool,
}

#[derive(ClapArgs, Debug)]
struct AskArgs {
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

    /// Continue the most recent session in this working directory.
    /// Mutually exclusive with --resume.
    #[arg(short = 'c', long = "continue", conflicts_with = "resume")]
    continue_session: bool,

    /// Resume a specific session by id.
    #[arg(long, value_name = "ID")]
    resume: Option<String>,

    /// Branch the resumed session into a new one instead of appending
    /// to it. Requires --resume. Useful for "what if I asked it this
    /// instead" experiments without polluting the original transcript.
    #[arg(long, requires = "resume")]
    fork: bool,

    /// Open an interactive fuzzy-filter picker over recent sessions
    /// and resume the one you select. Requires a TTY. Mutually
    /// exclusive with -c / --resume.
    #[arg(long, conflicts_with_all = ["continue_session", "resume"])]
    pick: bool,

    /// Restrict claude to read-only tools (Read, Glob, Grep). Good
    /// default for "summarize / explain this code" prompts where you
    /// don't want any edits or shell access. Mutually exclusive with
    /// --full-auto.
    #[arg(long, conflicts_with = "full_auto")]
    readonly: bool,

    /// Bypass all tool permission checks. Equivalent to claude's
    /// --dangerously-skip-permissions. Only use in a sandbox you
    /// trust. Mutually exclusive with --readonly.
    #[arg(long)]
    full_auto: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(SubCommand::History(args)) => run_history(args),
        Some(SubCommand::Last(args)) => run_last(args),
        None => run_ask(cli.ask).await,
    }
}

async fn run_ask(mut args: AskArgs) -> Result<()> {
    if args.pick {
        let id = pick_session_interactive()?;
        eprintln!("resuming session {}", id.get(..8).unwrap_or(&id));
        args.resume = Some(id);
    }
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

    let result = apply_session(QueryCommand::new(prompt), &args)
        .execute_json(&claude)
        .await?;

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
}

fn should_show_footer(args: &AskArgs) -> bool {
    !args.quiet && std::io::stderr().is_terminal()
}

fn run_history(args: HistoryArgs) -> Result<()> {
    use claude_wrapper::history::{HistoryRoot, ListOptions, ListSort};

    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    let limit = if args.all {
        None
    } else {
        Some(args.limit.unwrap_or(10))
    };
    let opts = ListOptions {
        limit,
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let sessions = root
        .list_sessions_with(args.project.as_deref(), &opts)
        .context("reading session history")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }

    if sessions.is_empty() {
        eprintln!("no sessions found");
        return Ok(());
    }

    println!(
        "{:<10} {:<17} {:>5}  {}",
        "SESSION", "LAST", "MSGS", "TITLE"
    );
    for s in &sessions {
        let short_id = s.session_id.get(..8).unwrap_or(&s.session_id);
        let last = s
            .last_timestamp
            .as_deref()
            .and_then(format_timestamp)
            .unwrap_or_else(|| "?".to_string());
        let title = s
            .title
            .as_deref()
            .or(s.first_user_preview.as_deref())
            .unwrap_or("(no title)");
        let title = truncate_arg(title, 60);
        println!(
            "{:<10} {:<17} {:>5}  {}",
            short_id, last, s.message_count, title
        );
    }
    Ok(())
}

fn format_timestamp(raw: &str) -> Option<String> {
    let truncated = raw.get(..16)?;
    Some(truncated.replace('T', " "))
}

fn run_last(args: LastArgs) -> Result<()> {
    use claude_wrapper::history::{HistoryEntry, HistoryRoot, ListOptions, ListSort};

    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    let opts = ListOptions {
        limit: Some(1),
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let sessions = root
        .list_sessions_with(args.project.as_deref(), &opts)
        .context("reading session history")?;
    let summary = sessions.first().ok_or_else(|| {
        anyhow::anyhow!("no sessions found")
    })?;
    let log = root
        .read_session(&summary.session_id)
        .context("reading most recent session")?;

    let last_assistant = log
        .entries
        .iter()
        .rev()
        .find_map(|entry| match entry {
            HistoryEntry::Assistant { message, .. } => Some(message),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("session has no assistant entries"))?;

    if let Some(text) = extract_message_text(last_assistant) {
        println!("{text}");
    } else {
        eprintln!("(last assistant entry had no text content)");
    }

    if std::io::stderr().is_terminal() {
        let short = summary.session_id.get(..8).unwrap_or(&summary.session_id);
        let when = summary
            .last_timestamp
            .as_deref()
            .and_then(format_timestamp)
            .unwrap_or_else(|| "?".to_string());
        eprintln!();
        eprintln!(
            "session {short} . {} messages . {when}",
            summary.message_count
        );
    }
    Ok(())
}

fn extract_message_text(message: &serde_json::Value) -> Option<String> {
    let blocks = message.get("content")?.as_array()?;
    let mut out = String::new();
    for block in blocks {
        if block.get("type").and_then(|t| t.as_str()) == Some("text")
            && let Some(text) = block.get("text").and_then(|v| v.as_str())
        {
            out.push_str(text);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn pick_session_interactive() -> Result<String> {
    use claude_wrapper::history::{HistoryRoot, ListOptions, ListSort};
    use dialoguer::{FuzzySelect, theme::ColorfulTheme};

    if !std::io::stdout().is_terminal() {
        bail!("--pick requires a TTY");
    }
    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    let opts = ListOptions {
        limit: Some(50),
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let sessions = root
        .list_sessions_with(None, &opts)
        .context("reading session history")?;
    if sessions.is_empty() {
        bail!("no sessions to pick from");
    }
    let items: Vec<String> = sessions
        .iter()
        .map(|s| {
            let id = s.session_id.get(..8).unwrap_or(&s.session_id);
            let when = s
                .last_timestamp
                .as_deref()
                .and_then(format_timestamp)
                .unwrap_or_else(|| "?".to_string());
            let title = s
                .title
                .as_deref()
                .or(s.first_user_preview.as_deref())
                .unwrap_or("(no title)");
            format!("{id}  {when}  {}", truncate_arg(title, 60))
        })
        .collect();
    let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Pick a session to resume")
        .items(&items)
        .default(0)
        .interact()
        .context("session picker cancelled")?;
    Ok(sessions[selection].session_id.clone())
}

fn apply_session(mut cmd: QueryCommand, args: &AskArgs) -> QueryCommand {
    if args.continue_session {
        cmd = cmd.continue_session();
    }
    if let Some(id) = &args.resume {
        cmd = cmd.resume(id.clone());
    }
    if args.fork {
        cmd = cmd.fork_session();
    }
    apply_permissions(cmd, args)
}

fn apply_permissions(mut cmd: QueryCommand, args: &AskArgs) -> QueryCommand {
    if args.readonly {
        cmd = cmd.allowed_tools(["Read", "Glob", "Grep"]);
    }
    if args.full_auto {
        cmd = cmd.dangerously_skip_permissions();
    }
    cmd
}

async fn run_streaming(claude: &Claude, prompt: String, args: &AskArgs) -> Result<()> {
    let cmd = apply_session(QueryCommand::new(prompt), args).output_format(OutputFormat::StreamJson);
    let show_meta = should_show_footer(args);
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
            handle_assistant_blocks(&event.data, show_meta, &mut tool_counts);
        }
    })
    .await?;
    println!();

    if show_meta
        && let Some(qr) = &final_result
    {
        eprintln!();
        if looks_like_refusal(&qr.result) {
            eprintln!("warning: response looks like a refusal");
        }
        if !tool_counts.is_empty() {
            eprintln!("used: {}", format_tool_summary(&tool_counts));
        }
        eprintln!("{}", format_footer(qr));
    }
    Ok(())
}

fn handle_assistant_blocks(
    data: &serde_json::Value,
    show_meta: bool,
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
                    print!("{text}");
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
                    eprintln!("> {}", summarize_tool(&name, input));
                }
            }
            _ => {}
        }
    }
}

fn format_tool_summary(counts: &HashMap<String, usize>) -> String {
    let mut sorted: Vec<(&String, &usize)> = counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    sorted
        .iter()
        .map(|(k, v)| format!("{k} x{v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn summarize_tool(name: &str, input: &serde_json::Value) -> String {
    let primary = ["file_path", "command", "pattern", "path", "url", "query"]
        .iter()
        .find_map(|k| input.get(k).and_then(|v| v.as_str()));
    match primary {
        Some(arg) => format!("{name}({})", truncate_arg(arg, 60)),
        None => name.to_string(),
    }
}

fn truncate_arg(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
        out.push_str("...");
        out
    }
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
