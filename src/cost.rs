//! `cwr cost` -- aggregate token usage from session history.
//!
//! Reads `~/.claude/projects/<slug>/<session>.jsonl` summaries via
//! `claude_wrapper::history::HistoryRoot` and rolls them up by total
//! or by project.
//!
//! Dollar amounts are intentionally not shown: claude-code persists
//! token counts to JSONL but not cost. Computing $$ requires a
//! per-model rate table -- planned as a follow-up. For now the
//! "cost" report is really a usage report.

use anyhow::{Context, Result};
use claude_wrapper::history::SessionSummary;
use serde::Serialize;
use std::collections::HashMap;

use crate::cli::CostArgs;
use crate::output::format_count;

/// Top-level rollup of session activity.
#[derive(Debug, Serialize)]
pub struct Rollup {
    pub sessions: usize,
    pub messages: usize,
    pub total_tokens: u64,
    pub projects: Vec<ProjectRollup>,
}

/// Per-project rollup. Sorted by total_tokens descending in `cwr cost
/// --by-project` output.
#[derive(Debug, Serialize)]
pub struct ProjectRollup {
    pub slug: String,
    pub sessions: usize,
    pub messages: usize,
    pub total_tokens: u64,
}

/// Entry point: dispatch the `cwr cost` subcommand.
pub fn run(args: CostArgs) -> Result<()> {
    use claude_wrapper::history::{HistoryRoot, ListOptions, ListSort};

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

    let rollup = aggregate(&sessions);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&rollup)?);
        return Ok(());
    }

    if args.by_project {
        print_by_project(&rollup, args.limit.unwrap_or(10));
    } else {
        print_totals(&rollup);
    }
    Ok(())
}

/// Aggregate a slice of [`SessionSummary`] into both an overall
/// total and a per-project breakdown.
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
            });
        entry.sessions += 1;
        entry.messages += s.message_count;
        entry.total_tokens += tokens;
    }

    let mut projects: Vec<ProjectRollup> = per_project.into_values().collect();
    projects.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens).then(a.slug.cmp(&b.slug)));

    Rollup {
        sessions: total_sessions,
        messages: total_messages,
        total_tokens,
        projects,
    }
}

fn print_totals(r: &Rollup) {
    println!("sessions:  {}", r.sessions);
    println!("messages:  {}", r.messages);
    println!("tokens:    {}", format_count(r.total_tokens));
    println!();
    println!("note: dollar amounts not yet shown -- claude persists tokens but not cost per session.");
    println!("      run with --by-project for a breakdown, or --json for machine output.");
}

fn print_by_project(r: &Rollup, limit: usize) {
    println!("sessions:  {}", r.sessions);
    println!("messages:  {}", r.messages);
    println!("tokens:    {} (across {} projects)", format_count(r.total_tokens), r.projects.len());
    println!();
    println!("{:>5}  {:>9}  {:>9}  PROJECT", "SES", "MSGS", "TOKENS");
    let cap = if limit == 0 { r.projects.len() } else { limit };
    for p in r.projects.iter().take(cap) {
        println!(
            "{:>5}  {:>9}  {:>9}  {}",
            p.sessions,
            p.messages,
            format_count(p.total_tokens),
            truncate_slug(&p.slug, 60),
        );
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
}
