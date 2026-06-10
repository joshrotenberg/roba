//! `roba cost` -- aggregate token usage from session history.
//!
//! Reads `~/.claude/projects/<slug>/<session>.jsonl` summaries via
//! `claude_wrapper::history::HistoryRoot` and rolls them up by total
//! or by project.
//!
//! Dollar amounts come from a bundled per-model rate table (see
//! [`crate::rates`]). `SessionSummary` (claude-wrapper 0.10+) exposes a
//! `total_cost_usd: Option<f64>` field, but roba still does a second
//! JSONL pass to recover the per-model and per-bucket (input / output /
//! cache-read / cache-write) breakdown needed for `Rollup.usage`,
//! `unknown_models`, and per-project dollar figures. The summary total
//! alone cannot replace that pass. The rates carry an `as_of` date
//! surfaced in the report; `--no-dollars` (or a stale-table override
//! via `--rates-file`) covers the case where the bundled numbers can't
//! be trusted.

use anyhow::{Context, Result};
use claude_wrapper::history::{HistoryRoot, SessionSummary};
use serde::Serialize;
use std::collections::HashMap;

use crate::cli::CostArgs;
use crate::output::format_count;
use crate::rates::Rates;

/// Per-token-type usage, kept split so differentiated input / output /
/// cache rates apply. Accumulated per model before costing.
#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    /// Sum across every bucket -- matches the wrapper's `total_tokens`.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// Top-level rollup of session activity.
#[derive(Debug, Serialize)]
pub struct Rollup {
    pub sessions: usize,
    pub messages: usize,
    pub total_tokens: u64,
    /// Per-token-type breakdown. Zero across the board when dollars are
    /// disabled (the log-reading pass that fills it is skipped).
    #[serde(skip_serializing_if = "usage_is_zero")]
    pub usage: Usage,
    /// Computed dollar cost across all sessions, or `None` when dollars
    /// are disabled or no session used a model present in the rate
    /// table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Model ids seen in the history that the rate table didn't cover,
    /// sorted and deduped. Surfaced so the user knows the dollar total
    /// is partial.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown_models: Vec<String>,
    pub projects: Vec<ProjectRollup>,
}

/// Per-project rollup. Sorted by total_tokens descending in `roba cost
/// --by-project` output.
#[derive(Debug, Serialize)]
pub struct ProjectRollup {
    pub slug: String,
    pub sessions: usize,
    pub messages: usize,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

fn usage_is_zero(u: &Usage) -> bool {
    *u == Usage::default()
}

/// Entry point: dispatch the `roba cost` subcommand.
pub fn run(args: CostArgs) -> Result<()> {
    use claude_wrapper::history::{ListOptions, ListSort};

    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    let opts = ListOptions {
        limit: None,
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let sessions = root
        .list_sessions_with(args.project.as_deref(), &opts)
        .context("reading session history")?;

    let mut rollup = aggregate(&sessions);

    // Dollars are on by default; `--no-dollars` (or ROBA_NO_DOLLARS)
    // opts out, in which case we skip both the rate-table load and the
    // per-session log read that costs it.
    let no_dollars = args.no_dollars || env_truthy("ROBA_NO_DOLLARS");
    let rates = if no_dollars {
        None
    } else {
        Some(Rates::resolve(args.rates_file.as_deref())?)
    };

    if let Some(rates) = &rates {
        enrich_costs(&mut rollup, &root, &sessions, rates);
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::VersionedResult::new(&rollup))?
        );
        return Ok(());
    }

    // The rates disclaimer goes to stderr so it never pollutes a
    // captured table; the numbers themselves go to stdout.
    if let Some(rates) = &rates {
        eprintln!("rates as of {} -- {}", rates.meta.as_of, rates.meta.source);
    }

    if args.by_project {
        print_by_project(&rollup, args.limit.unwrap_or(10), rates.is_some());
    } else {
        print_totals(&rollup, rates.is_some());
    }
    Ok(())
}

/// Aggregate a slice of [`SessionSummary`] into both an overall
/// total and a per-project breakdown. Token-only; dollar enrichment is
/// a separate pass (`enrich_costs`).
pub fn aggregate(sessions: &[SessionSummary]) -> Rollup {
    let mut per_project: HashMap<String, ProjectRollup> = HashMap::new();
    let mut total_sessions = 0usize;
    let mut total_messages = 0usize;
    let mut total_tokens: u64 = 0;

    for s in sessions {
        total_sessions += 1;
        total_messages += s.message_count;
        let tokens = s.total_tokens.unwrap_or(0);
        total_tokens += tokens;

        let entry = per_project
            .entry(s.project_slug.clone())
            .or_insert_with(|| ProjectRollup {
                slug: s.project_slug.clone(),
                sessions: 0,
                messages: 0,
                total_tokens: 0,
                cost_usd: None,
            });
        entry.sessions += 1;
        entry.messages += s.message_count;
        entry.total_tokens += tokens;
    }

    let mut projects: Vec<ProjectRollup> = per_project.into_values().collect();
    projects.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then(a.slug.cmp(&b.slug))
    });

    Rollup {
        sessions: total_sessions,
        messages: total_messages,
        total_tokens,
        usage: Usage::default(),
        cost_usd: None,
        unknown_models: Vec::new(),
        projects,
    }
}

/// Second pass: read each session's full JSONL to recover the
/// per-model token breakdown Claude Code records (`message.usage` +
/// `message.model`), cost it against `rates`, and fill the dollar
/// fields on the rollup and its projects.
///
/// Note: `SessionSummary.total_cost_usd` (available since
/// claude-wrapper 0.10 / bumped to 0.11 in this version) provides a
/// total USD figure per session, but it does not give per-model or
/// per-bucket (input/output/cache) detail. This JSONL pass is still
/// required to populate `Rollup.usage`, `unknown_models`, and
/// per-project cost; `total_cost_usd` does not replace it.
///
/// File reads are best-effort: an unreadable session is skipped (its
/// tokens already counted in [`aggregate`] from the cheap summary).
fn enrich_costs(
    rollup: &mut Rollup,
    root: &HistoryRoot,
    sessions: &[SessionSummary],
    rates: &Rates,
) {
    // model -> usage, accumulated globally and per project slug.
    let mut global: HashMap<String, Usage> = HashMap::new();
    let mut per_project: HashMap<String, HashMap<String, Usage>> = HashMap::new();

    for s in sessions {
        let path = root
            .path()
            .join(&s.project_slug)
            .join(format!("{}.jsonl", s.session_id));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let session_usage = usage_by_model(&text);
        let proj = per_project.entry(s.project_slug.clone()).or_default();
        for (model, u) in session_usage {
            global.entry(model.clone()).or_default().add(&u);
            proj.entry(model).or_default().add(&u);
        }
    }

    let (usage, cost, unknown) = cost_breakdown(&global, rates);
    rollup.usage = usage;
    rollup.cost_usd = cost;
    rollup.unknown_models = unknown;

    for p in &mut rollup.projects {
        if let Some(by_model) = per_project.get(&p.slug) {
            let (_, cost, _) = cost_breakdown(by_model, rates);
            p.cost_usd = cost;
        }
    }
}

/// Parse a session's raw JSONL, summing each assistant entry's
/// `message.usage` into a per-model [`Usage`] map. Pure over the text
/// so it unit-tests without touching disk.
pub fn usage_by_model(jsonl: &str) -> HashMap<String, Usage> {
    use serde_json::Value;
    let mut out: HashMap<String, Usage> = HashMap::new();
    for line in jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = v.get("message") else {
            continue;
        };
        let Some(u) = message.get("usage") else {
            continue;
        };
        let usage = Usage {
            input: u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            output: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            cache_read: u
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_write: u
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        };
        if usage.total() == 0 {
            continue;
        }
        let model = message
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        out.entry(model).or_default().add(&usage);
    }
    out
}

/// Reduce a per-model usage map to a combined [`Usage`] breakdown, a
/// total dollar cost (`None` when no model matched the table), and the
/// sorted/deduped list of model ids the table didn't cover.
pub fn cost_breakdown(
    by_model: &HashMap<String, Usage>,
    rates: &Rates,
) -> (Usage, Option<f64>, Vec<String>) {
    let mut usage = Usage::default();
    let mut total = 0.0f64;
    let mut any_known = false;
    let mut unknown: Vec<String> = Vec::new();

    for (model, u) in by_model {
        usage.add(u);
        match rates.cost_usd(model, u.input, u.output, u.cache_read, u.cache_write) {
            Some(c) => {
                total += c;
                any_known = true;
            }
            None => unknown.push(model.clone()),
        }
    }

    unknown.sort();
    unknown.dedup();
    (usage, any_known.then_some(total), unknown)
}

/// Format an optional dollar amount for the cost table: `$3.60`, or a
/// dash when no rate covered the row.
fn format_dollars(v: Option<f64>) -> String {
    match v {
        Some(v) => format!("${v:.2}"),
        None => "-".to_string(),
    }
}

/// Truthy means `1`/`true`/`yes`/`on` (case-insensitive). Mirrors the
/// env layer's bool semantics for `ROBA_NO_DOLLARS`.
fn env_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(s) => matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

fn print_totals(r: &Rollup, dollars: bool) {
    println!("sessions:  {}", r.sessions);
    println!("messages:  {}", r.messages);
    println!("tokens:    {}", format_count(r.total_tokens));
    if dollars {
        println!("cost:      {}", format_dollars(r.cost_usd));
        if !r.unknown_models.is_empty() {
            println!(
                "           (rates unknown for: {})",
                r.unknown_models.join(", ")
            );
        }
    } else {
        println!();
        println!("note: dollars suppressed (--no-dollars). tokens only.");
    }
    println!();
    println!("      run with --by-project for a breakdown, or --json for machine output.");
}

fn print_by_project(r: &Rollup, limit: usize, dollars: bool) {
    println!("sessions:  {}", r.sessions);
    println!("messages:  {}", r.messages);
    println!(
        "tokens:    {} (across {} projects)",
        format_count(r.total_tokens),
        r.projects.len()
    );
    if dollars {
        println!("cost:      {}", format_dollars(r.cost_usd));
        if !r.unknown_models.is_empty() {
            println!(
                "           (rates unknown for: {})",
                r.unknown_models.join(", ")
            );
        }
    }
    println!();
    if dollars {
        println!(
            "{:>5}  {:>9}  {:>9}  {:>10}  PROJECT",
            "SES", "MSGS", "TOKENS", "COST"
        );
    } else {
        println!("{:>5}  {:>9}  {:>9}  PROJECT", "SES", "MSGS", "TOKENS");
    }
    let cap = if limit == 0 { r.projects.len() } else { limit };
    for p in r.projects.iter().take(cap) {
        if dollars {
            println!(
                "{:>5}  {:>9}  {:>9}  {:>10}  {}",
                p.sessions,
                p.messages,
                format_count(p.total_tokens),
                format_dollars(p.cost_usd),
                truncate_slug(&p.slug, 60),
            );
        } else {
            println!(
                "{:>5}  {:>9}  {:>9}  {}",
                p.sessions,
                p.messages,
                format_count(p.total_tokens),
                truncate_slug(&p.slug, 60),
            );
        }
    }
    let rest = r.projects.len().saturating_sub(cap);
    if rest > 0 {
        println!("... and {rest} more (use -n 0 to see all)");
    }
}

fn truncate_slug(s: &str, max: usize) -> String {
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

    fn sess(slug: &str, msgs: usize, tokens: Option<u64>) -> SessionSummary {
        SessionSummary {
            session_id: format!("id-{slug}-{msgs}"),
            project_slug: slug.to_string(),
            message_count: msgs,
            first_timestamp: Some("2026-05-21T10:00:00.000Z".to_string()),
            last_timestamp: Some("2026-05-21T10:30:00.000Z".to_string()),
            title: None,
            first_user_preview: None,
            total_cost_usd: None,
            total_tokens: tokens,
            size_bytes: 0,
        }
    }

    #[test]
    fn aggregate_empty_input_returns_zeroes() {
        let r = aggregate(&[]);
        assert_eq!(r.sessions, 0);
        assert_eq!(r.messages, 0);
        assert_eq!(r.total_tokens, 0);
        assert!(r.projects.is_empty());
        assert!(r.cost_usd.is_none());
    }

    #[test]
    fn aggregate_sums_across_sessions() {
        let sessions = vec![
            sess("-Users-foo", 5, Some(100)),
            sess("-Users-foo", 3, Some(50)),
            sess("-Users-bar", 7, Some(200)),
        ];
        let r = aggregate(&sessions);
        assert_eq!(r.sessions, 3);
        assert_eq!(r.messages, 15);
        assert_eq!(r.total_tokens, 350);
    }

    #[test]
    fn aggregate_groups_by_project_sorted_by_tokens_desc() {
        let sessions = vec![
            sess("-aaa", 1, Some(100)),
            sess("-bbb", 1, Some(500)),
            sess("-ccc", 1, Some(300)),
        ];
        let r = aggregate(&sessions);
        let slugs: Vec<&str> = r.projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, vec!["-bbb", "-ccc", "-aaa"]);
    }

    #[test]
    fn aggregate_treats_missing_tokens_as_zero() {
        let sessions = vec![sess("-x", 5, None), sess("-x", 5, Some(40))];
        let r = aggregate(&sessions);
        assert_eq!(r.total_tokens, 40);
        assert_eq!(r.projects.len(), 1);
        assert_eq!(r.projects[0].total_tokens, 40);
    }

    #[test]
    fn aggregate_tie_breaks_by_slug_ascending() {
        let sessions = vec![sess("-zzz", 1, Some(100)), sess("-aaa", 1, Some(100))];
        let r = aggregate(&sessions);
        let slugs: Vec<&str> = r.projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, vec!["-aaa", "-zzz"]);
    }

    // -- usage_by_model ----------------------------------------------------

    fn assistant_line(model: &str, input: u64, output: u64, cr: u64, cw: u64) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":{cr},"cache_creation_input_tokens":{cw}}}}}}}"#
        )
    }

    #[test]
    fn usage_by_model_sums_per_model() {
        let jsonl = format!(
            "{}\n{}\n{}\n",
            assistant_line("claude-sonnet-4-6", 100, 50, 10, 5),
            assistant_line("claude-sonnet-4-6", 100, 50, 0, 0),
            assistant_line("claude-haiku-4-5", 200, 80, 0, 0),
        );
        let map = usage_by_model(&jsonl);
        let sonnet = map.get("claude-sonnet-4-6").unwrap();
        assert_eq!(sonnet.input, 200);
        assert_eq!(sonnet.output, 100);
        assert_eq!(sonnet.cache_read, 10);
        assert_eq!(sonnet.cache_write, 5);
        let haiku = map.get("claude-haiku-4-5").unwrap();
        assert_eq!(haiku.input, 200);
        assert_eq!(haiku.output, 80);
    }

    #[test]
    fn usage_by_model_skips_non_assistant_and_malformed() {
        let jsonl = format!(
            "{}\n{}\n{}\n{}\n",
            r#"{"type":"user","message":{"content":"hi"}}"#,
            "not json at all",
            r#"{"type":"assistant","message":{"model":"m","usage":{"input_tokens":0,"output_tokens":0}}}"#,
            assistant_line("claude-opus-4-5", 10, 20, 0, 0),
        );
        let map = usage_by_model(&jsonl);
        // user, malformed, and zero-token assistant all skipped.
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("claude-opus-4-5").unwrap().output, 20);
    }

    #[test]
    fn usage_by_model_missing_model_falls_back_to_unknown() {
        let jsonl =
            r#"{"type":"assistant","message":{"usage":{"input_tokens":10,"output_tokens":5}}}"#;
        let map = usage_by_model(jsonl);
        assert!(map.contains_key("unknown"));
    }

    // -- cost_breakdown ----------------------------------------------------

    #[test]
    fn cost_breakdown_sums_known_models() {
        let rates = Rates::bundled().unwrap();
        let mut by_model = HashMap::new();
        by_model.insert(
            "claude-sonnet-4-6".to_string(),
            Usage {
                input: 1_000_000,
                output: 1_000_000,
                cache_read: 0,
                cache_write: 0,
            },
        );
        let (usage, cost, unknown) = cost_breakdown(&by_model, &rates);
        assert_eq!(usage.input, 1_000_000);
        // $3 input + $15 output = $18.
        assert!((cost.unwrap() - 18.0).abs() < 1e-9);
        assert!(unknown.is_empty());
    }

    #[test]
    fn cost_breakdown_collects_unknown_models() {
        let rates = Rates::bundled().unwrap();
        let mut by_model = HashMap::new();
        by_model.insert(
            "mystery-model".to_string(),
            Usage {
                input: 100,
                output: 100,
                cache_read: 0,
                cache_write: 0,
            },
        );
        let (usage, cost, unknown) = cost_breakdown(&by_model, &rates);
        // Tokens still counted in the breakdown even when uncosted.
        assert_eq!(usage.input, 100);
        // No known model -> cost is None, not a misleading $0.
        assert!(cost.is_none());
        assert_eq!(unknown, vec!["mystery-model".to_string()]);
    }

    #[test]
    fn cost_breakdown_partial_known_sums_only_known() {
        let rates = Rates::bundled().unwrap();
        let mut by_model = HashMap::new();
        by_model.insert(
            "claude-sonnet-4-6".to_string(),
            Usage {
                input: 1_000_000,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
        );
        by_model.insert(
            "mystery".to_string(),
            Usage {
                input: 5,
                output: 5,
                cache_read: 0,
                cache_write: 0,
            },
        );
        let (_, cost, unknown) = cost_breakdown(&by_model, &rates);
        // Known sonnet input only: $3. Unknown model excluded but noted.
        assert!((cost.unwrap() - 3.0).abs() < 1e-9);
        assert_eq!(unknown, vec!["mystery".to_string()]);
    }

    #[test]
    fn format_dollars_renders_amount_or_dash() {
        assert_eq!(format_dollars(Some(3.6)), "$3.60");
        assert_eq!(format_dollars(Some(10.8)), "$10.80");
        assert_eq!(format_dollars(None), "-");
    }
}
