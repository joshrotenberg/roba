//! User-defined aliases: `git`-style shortcuts defined in `roba.toml`
//! that expand to a prompt template (plus default flags / a pinned
//! agent) and dispatch like a normal `roba` call.
//!
//! Two flavours, invoked identically as `roba NAME [args]`:
//!
//! - **Template alias** -- has a `template` field. Positional args
//!   substitute into `${1}` / `${pr}` / `${@}` placeholders, and
//!   `$(...)` spans run in the user's shell. The expansion becomes the
//!   prompt.
//! - **Flag-shortcut alias** -- no `template`. The user's positional
//!   args become the prompt verbatim; the alias just preloads
//!   `flags` / `agent`.
//!
//! # Lookup
//!
//! `roba <word> [args]` resolves in this order (see
//! [`crate::dispatch`]):
//!
//! 1. built-in subcommand -> dispatch as today
//! 2. alias in the merged config pool -> [`dispatch_alias`]
//! 3. otherwise -> error with close-match suggestions
//!
//! Clap captures the multi-arg form (`roba review 42`) via an
//! `external_subcommand` variant; the bare form (`roba commit-msg`)
//! is detected in [`crate::dispatch`] via [`bare_alias_candidate`].
//!
//! # Security
//!
//! `$(...)` substitution runs in the user's shell with the user's
//! permissions. Aliases come from user-controlled config, so this is
//! intentional -- it is *not* a sandbox. Don't define aliases whose
//! templates run commands you wouldn't run yourself.
//!
//! # v1 limitations
//!
//! - No recursive expansion: an alias cannot invoke another alias.
//! - Flags for an aliased call go *after* the positional args
//!   (`roba review 42 --full-auto`). Positional args come first.
//! - Alias `flags` are merged ahead of the user's CLI flags. Clap's
//!   normal precedence then applies: single-value flags are
//!   last-wins (user overrides), but mutually-exclusive flags (e.g.
//!   `--readonly` vs `--full-auto`) conflict and error rather than
//!   silently override.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::cli::{AliasAction, AskArgs};
use crate::profile::{self, Pool};

/// Built-in subcommand names. A user alias matching one of these is
/// shadowed (the built-in wins); [`profile::load_pool`] warns when it
/// loads such an alias.
pub const BUILTIN_SUBCOMMANDS: &[&str] = &[
    "history", "last", "profile", "cost", "skill", "agent", "alias", "help",
];

/// One `[alias.NAME]` section.
///
/// Either a *template alias* (`template` set) or a *flag-shortcut
/// alias* (`template` unset -- the user's args become the prompt).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Alias {
    /// Human-readable summary shown by `roba alias list`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Pin a claude-code subagent (equivalent to `flags = ["--agent",
    /// "NAME"]`; the dedicated field is for `roba alias list`
    /// discoverability).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Prompt body. Supports `${1}` / `${name}` / `${@}` variable
    /// substitution and `$(...)` shell substitution. Absent for a
    /// flag-shortcut alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Default CLI flags merged into the dispatch (before the user's
    /// own flags, which take precedence).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    /// Optional positional schema: names mapped to positional args by
    /// order, so `args = ["pr"]` makes `${pr}` resolve to the first
    /// positional argument.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

// ---------------------------------------------------------------------------
// Subcommand surface: roba alias {list,show,path}
// ---------------------------------------------------------------------------

/// Run a `roba alias <action>` subcommand.
pub fn run(action: AliasAction) -> Result<()> {
    match action {
        AliasAction::List => run_list(),
        AliasAction::Show { name } => run_show(&name),
        AliasAction::Path => run_path(),
    }
}

fn run_list() -> Result<()> {
    let pool = profile::load_pool()?;
    if pool.aliases.is_empty() {
        eprintln!("no aliases defined");
        if pool.sources.is_empty() {
            eprintln!("hint: add an `[alias.NAME]` section to a roba.toml");
        } else {
            eprintln!("sources checked:");
            for s in &pool.sources {
                eprintln!("  {}", s.display());
            }
        }
        return Ok(());
    }
    print!("{}", render_alias_list(&pool.aliases));
    Ok(())
}

/// Render the `alias list` table (assumes a non-empty map). The AGENT
/// column -- header and per-row value -- is shown only when at least one
/// alias pins an agent; with none, the table is just NAME / DESCRIPTION,
/// since an all-`-` column is pure noise. The returned string ends with a
/// trailing newline.
fn render_alias_list(aliases: &std::collections::HashMap<String, Alias>) -> String {
    let mut names: Vec<&String> = aliases.keys().collect();
    names.sort();
    let name_w = names.iter().map(|n| n.len()).max().unwrap_or(4).max(4);
    let desc_w = names
        .iter()
        .map(|n| aliases[*n].description.as_deref().unwrap_or("").len())
        .max()
        .unwrap_or(11)
        .max(11);
    let show_agent = aliases.values().any(|a| a.agent.is_some());

    let mut out = String::new();
    if show_agent {
        out.push_str(&format!(
            "{:<name_w$}  {:<desc_w$}  AGENT\n",
            "NAME", "DESCRIPTION"
        ));
    } else {
        out.push_str(&format!("{:<name_w$}  {}\n", "NAME", "DESCRIPTION"));
    }
    for name in names {
        let alias = &aliases[name];
        let desc = alias.description.as_deref().unwrap_or("");
        if show_agent {
            let agent = alias.agent.as_deref().unwrap_or("-");
            out.push_str(&format!("{name:<name_w$}  {desc:<desc_w$}  {agent}\n"));
        } else {
            out.push_str(&format!("{name:<name_w$}  {desc}\n"));
        }
    }
    out
}

fn run_show(name: &str) -> Result<()> {
    let pool = profile::load_pool()?;
    let alias = pool
        .aliases
        .get(name)
        .ok_or_else(|| anyhow::anyhow!(unknown_alias_message(name, &pool)))?;
    print!("{}", render_alias_toml(name, alias)?);
    if !alias.args.is_empty() {
        println!();
        println!("# positional schema: {}", alias.args.join(", "));
    }
    if let Some(template) = &alias.template {
        println!();
        println!("# expansion preview (variables as <placeholders>, shell left unexpanded):");
        print!("{}", preview_template(template, &alias.args));
        println!();
    }
    Ok(())
}

fn run_path() -> Result<()> {
    let pool = profile::load_pool()?;
    let user = profile::user_config_path();
    println!(
        "user:    {}",
        user.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    let cwd = std::env::current_dir().unwrap_or_default();
    let project = profile::discover_project_configs(&cwd);
    if project.is_empty() {
        println!("project: (none found above {})", cwd.display());
    } else {
        for (i, p) in project.iter().enumerate() {
            let label = if i == 0 { "project:" } else { "        " };
            println!("{label} {}", p.display());
        }
    }
    if !pool.sources.is_empty() {
        println!();
        println!(
            "loaded {} source(s); {} alias(es) defined:",
            pool.sources.len(),
            pool.aliases.len()
        );
        for s in &pool.sources {
            println!("  {}", s.display());
        }
    }
    Ok(())
}

/// Re-serialize one alias back to its `[alias.NAME]` TOML block for
/// `roba alias show`.
fn render_alias_toml(name: &str, alias: &Alias) -> Result<String> {
    use std::collections::HashMap;
    let mut wrapper: HashMap<String, HashMap<String, Alias>> = HashMap::new();
    let mut inner: HashMap<String, Alias> = HashMap::new();
    inner.insert(name.to_string(), alias.clone());
    wrapper.insert("alias".to_string(), inner);
    toml::to_string_pretty(&wrapper).context("re-serializing alias")
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Detect a bare single-word alias invocation (`roba commit-msg`).
///
/// Returns the alias name when the positional prompt is a single
/// whitespace-free word that matches an alias in the pool, and no
/// other prompt source (`-f` / `-e`) was used. Otherwise `None`, and
/// the caller proceeds with a normal prompt dispatch.
///
/// The whitespace check disambiguates a real prompt (`roba "explain
/// this"`) from an alias name; a single-word prompt that collides with
/// an alias name resolves in favour of the alias (documented).
pub fn bare_alias_candidate(ask: &AskArgs) -> Result<Option<String>> {
    let Some(prompt) = ask.prompt.as_deref() else {
        return Ok(None);
    };
    if prompt.is_empty() || prompt.chars().any(char::is_whitespace) {
        return Ok(None);
    }
    if ask.file.is_some() || ask.editor {
        return Ok(None);
    }
    let pool = profile::load_pool()?;
    if pool.aliases.contains_key(prompt) {
        Ok(Some(prompt.to_string()))
    } else {
        Ok(None)
    }
}

/// The argv tokens that follow `name` on the original command line.
/// Used for the bare-word case, where clap parsed any trailing flags
/// into the (discarded) first `AskArgs` rather than handing them back
/// as an external-subcommand tail.
pub fn trailing_args_from_env(name: &str) -> Vec<String> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    match all.iter().position(|a| a == name) {
        Some(pos) => all[pos + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// Expand an alias and dispatch it as a normal `roba` call.
///
/// `raw_args` are the tokens after the alias name: leading positional
/// args (consumed by the template) followed by any user flags.
pub async fn dispatch_alias(name: &str, raw_args: &[String]) -> Result<()> {
    use clap::Parser;

    let pool = profile::load_pool()?;
    let alias = match pool.aliases.get(name) {
        Some(a) => a.clone(),
        None => bail!(unknown_alias_message(name, &pool)),
    };

    let (positional, user_flags) = split_positional_flags(raw_args);
    let prompt = match &alias.template {
        Some(template) => expand_template(template, &alias.args, &positional)?,
        None => positional.join(" "),
    };

    // Synthetic argv: alias flags first, then the alias's pinned agent,
    // then the user's flags (later -> wins via clap), then the prompt
    // behind `--` so a leading dash or whitespace can't be reparsed as
    // a flag or subcommand.
    let mut argv: Vec<String> = vec!["roba".to_string()];
    argv.extend(alias.flags.iter().cloned());
    if let Some(agent) = &alias.agent {
        argv.push("--agent".to_string());
        argv.push(agent.clone());
    }
    argv.extend(user_flags.iter().cloned());
    if !prompt.is_empty() {
        argv.push("--".to_string());
        argv.push(prompt);
    }

    let cli = crate::cli::Cli::try_parse_from(&argv)
        .with_context(|| format!("expanding alias `{name}`"))?;
    crate::run_ask(cli.ask).await
}

/// Split alias trailing args into (positional, flags). Positional args
/// come first; the first token starting with `-` begins the flag tail,
/// and everything from there on is passed to clap verbatim.
fn split_positional_flags(raw: &[String]) -> (Vec<String>, Vec<String>) {
    let split = raw
        .iter()
        .position(|a| a.starts_with('-'))
        .unwrap_or(raw.len());
    (raw[..split].to_vec(), raw[split..].to_vec())
}

// ---------------------------------------------------------------------------
// Template expansion
// ---------------------------------------------------------------------------

/// Expand a template: `${...}` variables, `$$` escape, and `$(...)`
/// shell substitution, in a single left-to-right scan.
///
/// Variables resolve against `args` (the positional alias arguments):
/// `${@}` joins them, `${N}` selects the Nth (1-based), and a named
/// `${pr}` resolves via its index in `schema`. Unknown / out-of-range
/// variables expand to the empty string. `$$` emits a literal `$`.
/// `$(cmd)` runs `cmd` in `sh -c` and inserts its stdout (one trailing
/// newline trimmed). A literal `$` not part of any of these forms is
/// passed through unchanged -- so dollar amounts in prose survive.
pub fn expand_template(template: &str, schema: &[String], args: &[String]) -> Result<String> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            match chars[i + 1] {
                '$' => {
                    out.push('$');
                    i += 2;
                    continue;
                }
                '{' => {
                    if let Some(close) = find_char(&chars, i + 2, '}') {
                        let name: String = chars[i + 2..close].iter().collect();
                        out.push_str(&resolve_var(&name, schema, args));
                        i = close + 1;
                        continue;
                    }
                }
                '(' => {
                    if let Some(close) = find_matching_paren(&chars, i + 1) {
                        let cmd: String = chars[i + 2..close].iter().collect();
                        // Expand ${...}/$$/nested $() inside the command
                        // BEFORE handing it to the shell, so an alias arg
                        // like `$(gh pr diff ${pr})` sees the real value.
                        // `$$` stays the escape for shell-side `$`
                        // (`$(echo $$HOME)` reaches sh as `echo $HOME`).
                        let cmd = expand_template(&cmd, schema, args)?;
                        out.push_str(&run_shell(&cmd)?);
                        i = close + 1;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    Ok(out)
}

/// Resolve a `${...}` variable name against the positional args.
fn resolve_var(name: &str, schema: &[String], args: &[String]) -> String {
    if name == "@" {
        return args.join(" ");
    }
    if let Ok(n) = name.parse::<usize>()
        && n >= 1
    {
        return args.get(n - 1).cloned().unwrap_or_default();
    }
    if let Some(idx) = schema.iter().position(|s| s == name) {
        return args.get(idx).cloned().unwrap_or_default();
    }
    String::new()
}

/// Run `cmd` in `sh -c`, returning stdout with one trailing newline
/// trimmed (command-substitution convention). Errors carry the
/// command's stderr.
fn run_shell(cmd: &str) -> Result<String> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("running shell substitution `$({cmd})`"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("shell substitution `$({cmd})` failed: {}", stderr.trim());
    }
    let mut s = String::from_utf8_lossy(&output.stdout).into_owned();
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    Ok(s)
}

/// Find the first `target` char at or after `from`.
fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == target)
}

/// Given `open` is the index of a `(`, find the matching `)` by depth.
fn find_matching_paren(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, &c) in chars.iter().enumerate().skip(open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Render a template for `roba alias show`: variables become
/// `<placeholders>`, `$$` becomes `$`, and `$(...)` spans are left
/// untouched (never run during a preview).
fn preview_template(template: &str, schema: &[String]) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            match chars[i + 1] {
                '$' => {
                    out.push('$');
                    i += 2;
                    continue;
                }
                '{' => {
                    if let Some(close) = find_char(&chars, i + 2, '}') {
                        let name: String = chars[i + 2..close].iter().collect();
                        let _ = schema; // schema names are shown as-is
                        out.push('<');
                        out.push_str(if name == "@" { "args..." } else { &name });
                        out.push('>');
                        i = close + 1;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Unknown-alias diagnostics
// ---------------------------------------------------------------------------

/// Build the "no built-in or alias named X" error, with up to three
/// close matches (built-ins + aliases) by Levenshtein distance.
fn unknown_alias_message(name: &str, pool: &Pool) -> String {
    let mut candidates: Vec<String> = BUILTIN_SUBCOMMANDS.iter().map(|s| s.to_string()).collect();
    candidates.extend(pool.aliases.keys().cloned());
    let mut scored: Vec<(usize, String)> = candidates
        .into_iter()
        .map(|c| (levenshtein(name, &c), c))
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let suggestions: Vec<String> = scored
        .into_iter()
        .filter(|(d, _)| *d <= 3)
        .take(3)
        .map(|(_, c)| c)
        .collect();
    if suggestions.is_empty() {
        format!("no built-in or alias named `{name}`")
    } else {
        format!(
            "no built-in or alias named `{name}`; did you mean: {}?",
            suggestions.join(", ")
        )
    }
}

/// Classic dynamic-programming Levenshtein edit distance.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn expand_template_substitutes_positional() {
        let out = expand_template("PR #${1} and ${2}", &[], &s(&["42", "main"])).unwrap();
        assert_eq!(out, "PR #42 and main");
    }

    #[test]
    fn expand_template_substitutes_named() {
        let out = expand_template("PR #${pr}", &s(&["pr"]), &s(&["42"])).unwrap();
        assert_eq!(out, "PR #42");
    }

    #[test]
    fn expand_template_substitutes_at() {
        let out = expand_template("review ${@}", &[], &s(&["a", "b", "c"])).unwrap();
        assert_eq!(out, "review a b c");
    }

    #[test]
    fn expand_template_escapes_dollar() {
        let out = expand_template("cost is $$5.00", &[], &[]).unwrap();
        assert_eq!(out, "cost is $5.00");
    }

    #[test]
    fn expand_template_passes_lone_dollar_through() {
        // A `$` not starting a known form survives unchanged.
        let out = expand_template("price $5 each", &[], &[]).unwrap();
        assert_eq!(out, "price $5 each");
    }

    #[test]
    fn expand_template_unknown_var_is_empty() {
        let out = expand_template("[${nope}]", &[], &s(&["x"])).unwrap();
        assert_eq!(out, "[]");
    }

    #[test]
    fn expand_template_out_of_range_positional_is_empty() {
        let out = expand_template("[${3}]", &[], &s(&["only-one"])).unwrap();
        assert_eq!(out, "[]");
    }

    #[test]
    fn expand_template_runs_shell_substitution() {
        let out = expand_template("got: $(echo foo)", &[], &[]).unwrap();
        assert_eq!(out, "got: foo");
    }

    #[test]
    fn expand_template_shell_substitution_uses_args() {
        // The positional arg flows into the shell command: ${1} is
        // substituted BEFORE the $(...) is handed to the shell (#247).
        let out = expand_template("$(echo ${1})", &[], &["foo".into()]).unwrap();
        assert_eq!(out, "foo");
    }

    #[test]
    fn expand_template_shell_substitution_uses_named_args() {
        // The canonical sample-alias shape: $(gh pr diff ${pr}).
        let out = expand_template("$(echo pr=${pr})", &["pr".into()], &["42".into()]).unwrap();
        assert_eq!(out, "pr=42");
    }

    #[test]
    fn expand_template_shell_substitution_dollar_escape() {
        // $$ inside $(...) reaches the shell as a single $, so shell
        // variables stay expressible: $(X=hi; echo $$X) -> sh sees $X.
        let out = expand_template("$(X=hi; echo $$X)", &[], &[]).unwrap();
        assert_eq!(out, "hi");
    }

    #[test]
    fn expand_template_nested_parens_in_shell() {
        // Nested parens don't break the matcher; the inner $() is
        // expanded by the recursive pass, the outer by the shell.
        let out = expand_template("$(echo $(echo nested))", &[], &[]).unwrap();
        assert_eq!(out, "nested");
    }

    #[test]
    fn expand_template_failing_shell_errors() {
        let err = expand_template("$(exit 3)", &[], &[]).unwrap_err();
        assert!(format!("{err:#}").contains("shell substitution"));
    }

    #[test]
    fn split_positional_flags_splits_at_first_dash() {
        let (pos, flags) = split_positional_flags(&s(&["42", "main", "--readonly", "x"]));
        assert_eq!(pos, s(&["42", "main"]));
        assert_eq!(flags, s(&["--readonly", "x"]));
    }

    #[test]
    fn split_positional_flags_all_positional() {
        let (pos, flags) = split_positional_flags(&s(&["a", "b"]));
        assert_eq!(pos, s(&["a", "b"]));
        assert!(flags.is_empty());
    }

    #[test]
    fn split_positional_flags_all_flags() {
        let (pos, flags) = split_positional_flags(&s(&["--full-auto"]));
        assert!(pos.is_empty());
        assert_eq!(flags, s(&["--full-auto"]));
    }

    #[test]
    fn preview_template_shows_placeholders() {
        let out = preview_template("PR #${pr}: ${@} cost $$5 $(gh pr diff ${pr})", &s(&["pr"]));
        assert_eq!(out, "PR #<pr>: <args...> cost $5 $(gh pr diff <pr>)");
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("review", "reveiw"), 2);
        assert_eq!(levenshtein("cost", "cost"), 0);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn unknown_alias_suggests_close_match() {
        let mut pool = Pool::default();
        pool.aliases.insert("review".to_string(), Alias::default());
        let msg = unknown_alias_message("reveiw", &pool);
        assert!(msg.contains("no built-in or alias named `reveiw`"), "{msg}");
        assert!(msg.contains("review"), "{msg}");
    }

    #[test]
    fn unknown_alias_no_close_match_omits_suggestions() {
        let pool = Pool::default();
        let msg = unknown_alias_message("zzzzzzzz", &pool);
        assert_eq!(msg, "no built-in or alias named `zzzzzzzz`");
    }

    #[test]
    fn render_alias_toml_round_trips() {
        let alias = Alias {
            description: Some("Review a PR".to_string()),
            agent: Some("reviewer".to_string()),
            template: Some("PR #${pr}".to_string()),
            flags: s(&["--readonly"]),
            args: s(&["pr"]),
        };
        let rendered = render_alias_toml("review", &alias).unwrap();
        assert!(rendered.contains("[alias.review]"));
        assert!(rendered.contains("reviewer"));
        assert!(rendered.contains("--readonly"));
    }

    #[test]
    fn list_hides_agent_column_when_no_alias_pins_one() {
        let mut aliases = HashMap::new();
        aliases.insert(
            "cm".to_string(),
            Alias {
                description: Some("Commit message".to_string()),
                ..Alias::default()
            },
        );
        let out = render_alias_list(&aliases);
        assert!(out.contains("NAME"), "got:\n{out}");
        assert!(out.contains("DESCRIPTION"), "got:\n{out}");
        assert!(!out.contains("AGENT"), "got:\n{out}");
        assert!(!out.contains(" -"), "no stray agent dash: \n{out}");
    }

    #[test]
    fn list_shows_agent_column_when_one_alias_pins_one() {
        let mut aliases = HashMap::new();
        aliases.insert(
            "cm".to_string(),
            Alias {
                description: Some("Commit message".to_string()),
                ..Alias::default()
            },
        );
        aliases.insert(
            "review".to_string(),
            Alias {
                description: Some("Review a PR".to_string()),
                agent: Some("reviewer".to_string()),
                ..Alias::default()
            },
        );
        let out = render_alias_list(&aliases);
        assert!(out.contains("AGENT"), "got:\n{out}");
        // The agented alias shows its agent; the agentless one shows `-`.
        assert!(out.contains("reviewer"), "got:\n{out}");
        assert!(
            out.lines()
                .any(|l| l.starts_with("cm") && l.trim_end().ends_with('-')),
            "agentless row should fill with `-`:\n{out}"
        );
    }
}
