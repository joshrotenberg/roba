//! `roba config lint` -- static checks over roba config (read-only).
//!
//! The generate->validate bookend of the draft verbs guarantees a config
//! *parses*; lint extends the bar toward "wouldn't immediately warn." It
//! runs every STATICALLY-knowable check over the discovered config pool
//! (default) or a single named file, and reports findings with a typed
//! exit (0 when no ERROR findings -- warnings are advisory and pass; 1 on
//! any error) in both human and `--json` modes.
//!
//! The checks, per config file in scope:
//!
//! 1. **Parse** (error) -- through roba's REAL per-file deserializer
//!    ([`profile::pool::parse_config_str`], `deny_unknown_fields` on every
//!    section). A parse error IS a finding; the remaining checks are
//!    skipped for that file.
//! 2. **Built-in shadowing** (error) -- an `[alias.NAME]` whose NAME is a
//!    built-in subcommand is shadowed by the built-in and never dispatches.
//! 3. **Pinned-agent existence** (error) -- an `agent = "NAME"` in a
//!    `[profile.*]` or `[alias.*]` whose agent file does not resolve
//!    locally.
//! 4. **Pinned-agent tool mismatch** (error, best-effort) -- a pinned agent
//!    that declares tools (Bash/Edit/Write/...) beyond what the entry's own
//!    flags would grant, surfaced with the intent-respecting hint. For a
//!    profile the posture maps from its typed fields; for an alias it is
//!    parsed from its `flags` (skipped when those flags don't parse).
//! 5. **Top-level unsafe settings** (warning) -- a top-level `worktree` or
//!    `full_auto` is a task-scoped knob that auto-applies to every run
//!    (top-level `worktree` also silently defeats `-c`/`--resume`); it
//!    belongs in a named `[profile.NAME]`. Advisory -- the config parses
//!    and works, so this does NOT fail the exit code.
//!
//! # Honest limits
//!
//! Lint-clean at one moment does not guarantee warning-free at run time
//! elsewhere: agent files, the surrounding pool, and env differ by
//! machine, and `$(...)` in an alias template evaluates at expansion
//! time, not here. The linter is a tripwire, not a proof.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::agent_check::{self, Posture};
use crate::aliases::{self, Alias};
use crate::cli::ConfigLintArgs;
use crate::profile::{self, Profile, WorktreeSetting};

/// Finding severity. `Error` fails the lint (exit 1); `Warning` is an
/// advisory that is surfaced but still passes (exit 0), mirroring how
/// `roba doctor` treats warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Error,
    Warning,
}

impl Severity {
    /// The severity keyword printed in the human report.
    fn keyword(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }

    /// The ANSI color for this severity's keyword: bold red for errors,
    /// bold yellow for warnings -- the same red/yellow `doctor` and
    /// `config explain` use (red = broken, yellow = advisory).
    fn color(self) -> &'static str {
        match self {
            Severity::Error => "\x1b[1;31m",
            Severity::Warning => "\x1b[1;33m",
        }
    }
}

/// One lint finding: which file, which rule, its severity, a
/// human-readable message, and an optional actionable hint. Serialized
/// into the `--json` envelope; the hint is omitted when absent.
#[derive(Debug, Serialize)]
struct Finding {
    file: String,
    rule: &'static str,
    severity: Severity,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

/// The `--json` `result` payload: the findings plus a top-level `ok`
/// flag. `ok` tracks the EXIT -- true exactly when there are no ERROR
/// findings, so it can be true while advisory warnings are present (lint
/// passed with advisories).
#[derive(Debug, Serialize)]
struct Report {
    findings: Vec<Finding>,
    ok: bool,
}

/// The process exit code for a finding set: 1 iff any finding is an
/// `Error`, else 0 (clean or warnings-only).
fn exit_code(findings: &[Finding]) -> i32 {
    if findings.iter().any(|f| f.severity == Severity::Error) {
        1
    } else {
        0
    }
}

/// Run `roba config lint [PATH] [--json]`. Returns the process exit code
/// (0 = clean, 1 = at least one finding); the same code is returned in
/// both human and `--json` modes.
pub fn run(args: ConfigLintArgs) -> Result<i32> {
    let cwd = std::env::current_dir().context("resolving current directory")?;
    let files = match &args.path {
        Some(p) => {
            if !p.is_file() {
                bail!("no such config file: {}", p.display());
            }
            vec![p.clone()]
        }
        None => pool_files(&cwd),
    };

    let mut findings = Vec::new();
    for file in &files {
        lint_file(file, &cwd, &mut findings);
    }
    let exit = exit_code(&findings);
    // `ok` tracks the EXIT (no error findings), so it stays true when only
    // advisory warnings are present.
    let ok = exit == 0;

    if args.json {
        let report = Report { findings, ok };
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::VersionedResult::new(&report))?
        );
        return Ok(exit);
    }

    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    print!("{}", render_human(&findings, &files, color));
    Ok(exit)
}

/// Wrap `s` in an ANSI `code` (with reset) when `on`, else return it
/// untouched. The one place lint decides color, so a non-TTY / `NO_COLOR`
/// run stays byte-plain (mirrors `doctor`'s gate-only approach for report
/// verbs -- no separate `--plain` flag).
fn paint(s: &str, code: &str, on: bool) -> String {
    if on {
        format!("{code}{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Cyan, matching the file/key color in `doctor` and `config explain`.
const FILE_COLOR: &str = "\x1b[36m";
/// Dim, matching the secondary-detail color in `config explain`.
const HINT_COLOR: &str = "\x1b[2m";

/// The config files in pool scope: the user config (if present) plus
/// every `roba.toml` in the walk up from `cwd`. Both sources are already
/// filtered to existing files.
fn pool_files(cwd: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(user) = profile::user_config_path()
        && user.is_file()
    {
        files.push(user);
    }
    files.extend(profile::discover_project_configs(cwd));
    files
}

/// Run every check over one config file, appending findings.
fn lint_file(path: &Path, cwd: &Path, findings: &mut Vec<Finding>) {
    let file = path.display().to_string();

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            findings.push(Finding {
                file,
                rule: "read",
                severity: Severity::Error,
                message: format!("could not read file: {e}"),
                hint: None,
            });
            return;
        }
    };

    // Check 1: parse through the real deserializer. A parse error is a
    // finding and short-circuits the rest (nothing structured to check).
    let cfg = match profile::pool::parse_config_str(&content) {
        Ok(c) => c,
        Err(e) => {
            findings.push(Finding {
                file,
                rule: "parse",
                severity: Severity::Error,
                message: format!("{e:#}"),
                hint: Some("fix the TOML / remove the unknown key so the file loads".to_string()),
            });
            return;
        }
    };

    // Check 5 (advisory): top-level unsafe settings. A `worktree`/`full_auto`
    // at the TOP LEVEL (`cfg.defaults`) auto-applies to every run in the
    // repo; the same key inside a `[profile.*]`/`[alias.*]` is fine (and is
    // not inspected here -- we only read `cfg.defaults`). Warnings, not
    // errors: the config parses and works, so this never fails the exit.
    if matches!(
        cfg.defaults.worktree,
        Some(WorktreeSetting::Enabled(true)) | Some(WorktreeSetting::Named(_))
    ) {
        findings.push(Finding {
            file: file.clone(),
            rule: "top-level-worktree",
            severity: Severity::Warning,
            message: "top-level `worktree` auto-applies to every run and silently defeats `-c`/`--resume`; move it to a named [profile.NAME]".to_string(),
            hint: Some(
                "e.g. put `worktree = true` under `[profile.worker]` and opt in with `--profile worker`"
                    .to_string(),
            ),
        });
    }
    if cfg.defaults.full_auto == Some(true) {
        findings.push(Finding {
            file: file.clone(),
            rule: "top-level-full-auto",
            severity: Severity::Warning,
            message: "top-level `full_auto` bypasses all permission checks on every run (safe-by-default off); move it to a named [profile.NAME]".to_string(),
            hint: Some(
                "e.g. put `full_auto = true` under `[profile.worker]` and opt in with `--profile worker`"
                    .to_string(),
            ),
        });
    }

    // Check 2: built-in shadowing -- an alias whose name is a built-in
    // subcommand is unreachable as a verb (the built-in wins the lookup).
    let mut alias_names: Vec<&String> = cfg.alias.keys().collect();
    alias_names.sort();
    for name in alias_names {
        if aliases::is_builtin_subcommand(name) {
            findings.push(Finding {
                file: file.clone(),
                rule: "builtin-shadow",
                severity: Severity::Error,
                message: format!(
                    "alias `{name}` is shadowed by the built-in `{name}` subcommand; it never dispatches"
                ),
                hint: Some("rename the alias to a verb that isn't a built-in".to_string()),
            });
        }
    }

    // Checks 3 + 4: pinned agents in profiles, then aliases.
    let mut profile_names: Vec<&String> = cfg.profile.keys().collect();
    profile_names.sort();
    for name in profile_names {
        let profile = &cfg.profile[name];
        if let Some(agent) = &profile.agent {
            check_pinned_agent(
                agent,
                &format!("profile.{name}"),
                Some(profile_posture(profile)),
                cwd,
                &file,
                findings,
            );
        }
    }

    let mut alias_keys: Vec<&String> = cfg.alias.keys().collect();
    alias_keys.sort();
    for name in alias_keys {
        let alias = &cfg.alias[name];
        if let Some(agent) = &alias.agent {
            check_pinned_agent(
                agent,
                &format!("alias.{name}"),
                alias_posture(alias),
                cwd,
                &file,
                findings,
            );
        }
    }
}

/// Check one pinned agent reference: that its file resolves (check 3),
/// and -- when a `posture` is known -- that its declared tools don't
/// exceed what that posture grants (check 4, best-effort).
fn check_pinned_agent(
    agent: &str,
    source: &str,
    posture: Option<Posture>,
    cwd: &Path,
    file: &str,
    findings: &mut Vec<Finding>,
) {
    let Some(agent_path) = agent_check::find_agent_file(agent, cwd) else {
        findings.push(Finding {
            file: file.to_string(),
            rule: "missing-agent",
            severity: Severity::Error,
            message: format!("{source}: pinned agent `{agent}` not found"),
            hint: Some(
                "check the name, or that the agent exists under .claude/agents/ (project or ~)"
                    .to_string(),
            ),
        });
        return;
    };

    // Check 4 needs a known posture AND a readable, tools-declaring agent
    // file. Any gap (unparsable flags, unreadable file, no `tools:` field)
    // is a best-effort skip, not a finding.
    let Some(posture) = posture else { return };
    let Ok(content) = std::fs::read_to_string(&agent_path) else {
        return;
    };
    let Some(declared) = agent_check::parse_tools(&content) else {
        return;
    };
    let missing = agent_check::missing_tools_for_posture(&declared, &posture);
    if !missing.is_empty() {
        findings.push(Finding {
            file: file.to_string(),
            rule: "agent-tool-mismatch",
            severity: Severity::Error,
            message: format!(
                "{source}: agent `{agent}` declares tools not granted by this entry's flags: [{}]",
                missing.join(", ")
            ),
            hint: Some(
                "intentional? add --no-agent-check to the entry's flags; otherwise grant via --allow-tool / --writable / --full-auto"
                    .to_string(),
            ),
        });
    }
}

/// The permission posture a `[profile.*]` produces, from its typed
/// permission fields (the three [`Posture`] reads).
fn profile_posture(p: &Profile) -> Posture {
    Posture {
        writable: p.writable.unwrap_or(false),
        full_auto: p.full_auto.unwrap_or(false),
        allow_tool: p.allow_tool.clone(),
    }
}

/// The permission posture an `[alias.*]` produces, parsed from its free-
/// form `flags` exactly as a real dispatch would (`Cli::try_parse_from`).
/// `None` when the flags don't parse (e.g. mutually-exclusive flags) --
/// the mismatch check is then skipped and the conflict surfaces at run
/// time, where clap reports it precisely.
fn alias_posture(alias: &Alias) -> Option<Posture> {
    use clap::Parser;
    let mut argv: Vec<String> = vec!["roba".to_string()];
    argv.extend(alias.flags.iter().cloned());
    // A placeholder positional so a flags-only alias still parses.
    argv.push("placeholder".to_string());
    crate::cli::Cli::try_parse_from(&argv)
        .ok()
        .map(|cli| Posture::from_args(&cli.ask))
}

/// Render the human findings list: a "no issues" line when clean, else
/// findings grouped by file with their hints. This verb's output IS the
/// report, so it goes to stdout, mirroring how `doctor` renders. Pure over
/// `(findings, files, color)` so it is tested without a TTY: the file
/// header is cyan, the severity keyword bold red/yellow, and the hint dim
/// when `color`, and byte-plain otherwise. The `[rule]` token, message, and
/// summary stay plain in both modes.
fn render_human(findings: &[Finding], files: &[PathBuf], color: bool) -> String {
    let mut out = String::new();
    if findings.is_empty() {
        if files.is_empty() {
            out.push_str("no roba.toml found in the config pool\n");
        } else {
            out.push_str(&format!(
                "no issues found ({} file(s) checked)\n",
                files.len()
            ));
        }
        return out;
    }

    let mut current: Option<&str> = None;
    for f in findings {
        if current != Some(f.file.as_str()) {
            out.push_str(&paint(&f.file, FILE_COLOR, color));
            out.push('\n');
            current = Some(f.file.as_str());
        }
        let sev = paint(f.severity.keyword(), f.severity.color(), color);
        out.push_str(&format!("  {sev}: [{}] {}\n", f.rule, f.message));
        if let Some(hint) = &f.hint {
            out.push_str(&format!(
                "    {}\n",
                paint(&format!("hint: {hint}"), HINT_COLOR, color)
            ));
        }
    }

    let n = findings.len();
    let errors = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let warnings = n - errors;
    let plural = if n == 1 { "" } else { "s" };
    let breakdown = format!(
        "{errors} error{}, {warnings} warning{}",
        if errors == 1 { "" } else { "s" },
        if warnings == 1 { "" } else { "s" }
    );
    if errors == 0 {
        // Warnings-only: lint passes (exit 0); say so explicitly.
        let noun = if n == 1 { "advisory" } else { "advisories" };
        out.push_str(&format!(
            "\n{n} {noun} found ({breakdown}); no errors -- lint passes\n"
        ));
    } else {
        out.push_str(&format!("\n{n} issue{plural} found ({breakdown})\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn findings_for(dir: &Path, content: &str) -> Vec<Finding> {
        let path = write(dir, "roba.toml", content);
        let mut findings = Vec::new();
        lint_file(&path, dir, &mut findings);
        findings
    }

    #[test]
    fn clean_config_has_no_findings() {
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(
            dir.path(),
            "readonly = true\n\n[profile.review]\ngit_diff = true\n\n[alias.r]\ntemplate = \"review ${@}\"\n",
        );
        assert!(findings.is_empty(), "expected clean, got: {findings:?}");
    }

    #[test]
    fn builtin_shadowing_alias_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(dir.path(), "[alias.show]\ntemplate = \"x ${@}\"\n");
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].rule, "builtin-shadow");
        assert!(findings[0].message.contains("show"), "{:?}", findings[0]);
    }

    #[test]
    fn another_builtin_shadowing_alias_is_flagged() {
        // `cost` is a real built-in subcommand.
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(dir.path(), "[alias.cost]\ntemplate = \"x ${@}\"\n");
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].rule, "builtin-shadow");
    }

    #[test]
    fn non_builtin_alias_is_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(dir.path(), "[alias.my-verb]\ntemplate = \"x ${@}\"\n");
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn parse_error_is_a_finding_and_short_circuits() {
        let dir = tempfile::tempdir().unwrap();
        // An unknown top-level key fails the real deny_unknown_fields
        // deserializer. The alias shadowing on the same file must NOT also
        // be reported (parse short-circuits).
        let findings = findings_for(
            dir.path(),
            "totally_bogus_key = true\n\n[alias.cost]\ntemplate = \"x\"\n",
        );
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].rule, "parse");
        assert!(
            findings[0].message.contains("totally_bogus_key")
                || findings[0].message.contains("unknown field"),
            "{:?}",
            findings[0]
        );
    }

    #[test]
    fn missing_pinned_agent_in_profile_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        // No .claude/agents/ tree under the tempdir, and HOME isn't set to
        // it, so the agent cannot resolve.
        let findings = findings_for(dir.path(), "[profile.x]\nagent = \"nope-not-here\"\n");
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].rule, "missing-agent");
        assert!(
            findings[0].message.contains("nope-not-here"),
            "{:?}",
            findings[0]
        );
        assert!(
            findings[0].message.contains("profile.x"),
            "{:?}",
            findings[0]
        );
    }

    #[test]
    fn present_pinned_agent_with_matching_tools_is_clean() {
        let dir = tempfile::tempdir().unwrap();
        // A project-local agent declaring only the read-only trio.
        std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();
        std::fs::write(
            dir.path().join(".claude/agents/reader.md"),
            "---\nname: reader\ntools: Read, Glob, Grep\n---\nbody\n",
        )
        .unwrap();
        let findings = findings_for(dir.path(), "[profile.x]\nagent = \"reader\"\n");
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn agent_tool_mismatch_in_readonly_profile_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();
        std::fs::write(
            dir.path().join(".claude/agents/writer.md"),
            "---\nname: writer\ntools: Read, Edit, Write, Bash\n---\nbody\n",
        )
        .unwrap();
        // A profile with no write grant: the agent's Edit/Write/Bash exceed
        // the read-only posture -> mismatch finding.
        let findings = findings_for(dir.path(), "[profile.x]\nagent = \"writer\"\n");
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].rule, "agent-tool-mismatch");
        assert!(findings[0].message.contains("Edit"), "{:?}", findings[0]);
    }

    #[test]
    fn agent_tool_mismatch_silenced_by_full_auto_profile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude/agents")).unwrap();
        std::fs::write(
            dir.path().join(".claude/agents/writer.md"),
            "---\nname: writer\ntools: Read, Edit, Write, Bash\n---\nbody\n",
        )
        .unwrap();
        // full_auto covers all tools -> no mismatch.
        let findings = findings_for(
            dir.path(),
            "[profile.x]\nfull_auto = true\nagent = \"writer\"\n",
        );
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    #[test]
    fn alias_posture_parses_writable_flag() {
        let alias = Alias {
            flags: vec!["--writable".to_string()],
            ..Alias::default()
        };
        let posture = alias_posture(&alias).expect("--writable parses");
        assert!(posture.writable);
        assert!(!posture.full_auto);
    }

    #[test]
    fn alias_posture_is_none_for_conflicting_flags() {
        // --readonly and --full-auto are mutually exclusive; clap rejects
        // them, so the mismatch check is skipped (None).
        let alias = Alias {
            flags: vec!["--readonly".to_string(), "--full-auto".to_string()],
            ..Alias::default()
        };
        assert!(alias_posture(&alias).is_none());
    }

    #[test]
    fn top_level_worktree_true_is_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(dir.path(), "worktree = true\n");
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].rule, "top-level-worktree");
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn top_level_named_worktree_is_also_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(dir.path(), "worktree = \"mybranch\"\n");
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].rule, "top-level-worktree");
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn nested_worktree_in_profile_is_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(dir.path(), "[profile.worker]\nworktree = true\n");
        assert!(
            findings.iter().all(|f| f.rule != "top-level-worktree"),
            "nested worktree must not be flagged: {findings:?}"
        );
    }

    #[test]
    fn top_level_full_auto_is_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(dir.path(), "full_auto = true\n");
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].rule, "top-level-full-auto");
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn nested_full_auto_in_profile_is_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(dir.path(), "[profile.worker]\nfull_auto = true\n");
        assert!(
            findings.iter().all(|f| f.rule != "top-level-full-auto"),
            "nested full_auto must not be flagged: {findings:?}"
        );
    }

    #[test]
    fn top_level_full_auto_false_is_not_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let findings = findings_for(dir.path(), "full_auto = false\n");
        assert!(
            findings.iter().all(|f| f.rule != "top-level-full-auto"),
            "a top-level false is pointless but not unsafe: {findings:?}"
        );
    }

    #[test]
    fn exit_code_distinguishes_errors_warnings_and_clean() {
        // Empty -> clean -> 0.
        assert_eq!(exit_code(&[]), 0);
        // Warnings-only -> still passes -> 0.
        let warnings = vec![Finding {
            file: "f".to_string(),
            rule: "top-level-worktree",
            severity: Severity::Warning,
            message: "w".to_string(),
            hint: None,
        }];
        assert_eq!(exit_code(&warnings), 0);
        // Any error -> 1.
        let with_error = vec![
            Finding {
                file: "f".to_string(),
                rule: "top-level-worktree",
                severity: Severity::Warning,
                message: "w".to_string(),
                hint: None,
            },
            Finding {
                file: "f".to_string(),
                rule: "parse",
                severity: Severity::Error,
                message: "e".to_string(),
                hint: None,
            },
        ];
        assert_eq!(exit_code(&with_error), 1);
    }

    fn finding(file: &str, rule: &'static str, severity: Severity, hint: Option<&str>) -> Finding {
        Finding {
            file: file.to_string(),
            rule,
            severity,
            message: "msg".to_string(),
            hint: hint.map(str::to_string),
        }
    }

    #[test]
    fn render_human_plain_leaks_no_ansi() {
        let findings = vec![
            finding("roba.toml", "parse", Severity::Error, Some("fix it")),
            finding(
                "roba.toml",
                "top-level-worktree",
                Severity::Warning,
                Some("move it"),
            ),
        ];
        let text = render_human(&findings, &[PathBuf::from("roba.toml")], false);
        assert!(!text.contains('\x1b'), "plain mode leaked ANSI:\n{text}");
        // The unpainted content is intact.
        assert!(text.contains("roba.toml"), "{text}");
        assert!(text.contains("  error: [parse] msg"), "{text}");
        assert!(text.contains("    hint: fix it"), "{text}");
    }

    #[test]
    fn render_human_clean_plain_leaks_no_ansi() {
        let text = render_human(&[], &[PathBuf::from("roba.toml")], false);
        assert!(!text.contains('\x1b'), "plain mode leaked ANSI:\n{text}");
        assert!(
            text.contains("no issues found (1 file(s) checked)"),
            "{text}"
        );
    }

    #[test]
    fn render_human_color_wraps_severity_header_and_hint() {
        let findings = vec![
            finding("roba.toml", "parse", Severity::Error, Some("fix it")),
            finding("roba.toml", "top-level-worktree", Severity::Warning, None),
        ];
        let text = render_human(&findings, &[PathBuf::from("roba.toml")], true);
        // File header cyan, error severity bold red, warning severity bold
        // yellow, hint dim -- the five-code standard palette.
        assert!(text.contains("\x1b[36mroba.toml\x1b[0m"), "{text}");
        assert!(text.contains("\x1b[1;31merror\x1b[0m"), "{text}");
        assert!(text.contains("\x1b[1;33mwarning\x1b[0m"), "{text}");
        assert!(text.contains("\x1b[2mhint: fix it\x1b[0m"), "{text}");
        // The `[rule]` token and summary stay plain.
        assert!(text.contains("[parse]"), "{text}");
        assert!(!text.contains("\x1b[36m[parse]"), "{text}");
    }

    #[test]
    fn severity_colors_are_the_standard_codes() {
        assert_eq!(Severity::Error.color(), "\x1b[1;31m");
        assert_eq!(Severity::Warning.color(), "\x1b[1;33m");
    }

    #[test]
    fn profile_posture_maps_typed_fields() {
        let p = Profile {
            writable: Some(true),
            allow_tool: vec!["Bash(git:*)".to_string()],
            ..Profile::default()
        };
        let posture = profile_posture(&p);
        assert!(posture.writable);
        assert!(!posture.full_auto);
        assert_eq!(posture.allow_tool, vec!["Bash(git:*)".to_string()]);
    }
}
