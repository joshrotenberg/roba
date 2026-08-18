//! Prompt input + composition.
//!
//! - Input sources: positional string, file (`-f`), editor (`-e`), stdin
//! - Composition: prepend, attachments (`--attach` globs), git context,
//!   main prompt, append
//! - Templating: `{{VAR}}` substitution
//!
//! `resolve_main_prompt` resolves a single input source; `compose_prompt`
//! assembles the final body with the prepend/attach/git slots wired in.

use anyhow::{Context, Result, bail};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::AskArgs;

/// The resolved prompt body plus any piped-stdin context to merge.
///
/// `main` is the prompt body resolved from one input source. `piped_context`
/// is non-`None` only when an *explicit* prompt was given (positional, `-p`,
/// or `-f`) AND stdin was piped with non-empty content: that content becomes
/// a context block so `cat err.log | roba "what's wrong?"` just works instead
/// of silently dropping the pipe.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ResolvedPrompt {
    /// The main prompt body (editor / file / positional / `-p` / stdin-as-prompt).
    pub main: Option<String>,
    /// Piped stdin to embed as context, when an explicit prompt is present.
    pub piped_context: Option<String>,
}

/// Resolve the main prompt body from one of: editor, file, positional
/// arg, stdin (piped or explicit `-`). `main` is `None` only when no
/// source was given and stdin is a TTY -- the caller should then rely on
/// prepend/append/attach to supply content, or error if those are also
/// empty.
///
/// When the prompt is *explicit* (file `-f`, positional, or `-p`) and
/// stdin is piped with non-empty content, that content is returned as
/// `piped_context` for the caller to merge as a context block. Empty or
/// whitespace-only piped stdin yields no context (byte-identical to no
/// pipe). Stdin is read at most once. The `-` positional and the
/// no-positional path keep their existing "stdin IS the prompt" meaning.
pub fn resolve_main_prompt(
    positional: Option<&str>,
    file: Option<&Path>,
    editor: bool,
    editor_history: Option<usize>,
) -> Result<ResolvedPrompt> {
    if editor {
        if !std::io::stdin().is_terminal() {
            bail!("--editor requires a TTY; pipe-mode input is incompatible");
        }
        let n = editor_history.unwrap_or(1);
        return Ok(ResolvedPrompt {
            main: Some(compose_in_editor(n)?),
            piped_context: None,
        });
    }
    if let Some(path) = file {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading prompt from {}", path.display()))?;
        let trimmed = content.trim_end().to_string();
        if trimmed.is_empty() {
            bail!("file {} is empty", path.display());
        }
        // `-f` is an explicit prompt: piped stdin merges as context.
        return Ok(ResolvedPrompt {
            main: Some(trimmed),
            piped_context: piped_stdin_context()?,
        });
    }
    match positional {
        Some("-") => Ok(ResolvedPrompt {
            main: Some(read_stdin()?),
            piped_context: None,
        }),
        Some(p) => Ok(ResolvedPrompt {
            // Explicit positional / `-p`: piped stdin merges as context.
            main: Some(p.to_string()),
            piped_context: piped_stdin_context()?,
        }),
        None => {
            if std::io::stdin().is_terminal() {
                Ok(ResolvedPrompt::default())
            } else {
                Ok(ResolvedPrompt {
                    main: Some(read_stdin()?),
                    piped_context: None,
                })
            }
        }
    }
}

/// When stdin is piped (not a TTY), read it and return its content as a
/// context block, or `None` for a TTY or empty/whitespace-only input.
/// This is the read shim; the keep/drop decision lives in the pure
/// `stdin_as_context` so it stays unit-testable.
fn piped_stdin_context() -> Result<Option<String>> {
    if std::io::stdin().is_terminal() {
        return Ok(None);
    }
    // Non-TTY stdin, but only read it if data is actually ready. An
    // open-but-idle inherited pipe (a backgrounded/orchestrated roba whose
    // stdin is an open pipe with no data and no EOF) would block
    // `read_to_string` forever (#288). The shared probe classifies it as
    // not-readable, so we skip it: no piped context, no hang. A pipe with
    // bytes or a non-empty `< file` redirect is readable and reads as before.
    // Only a DEFINITIVE "not readable" skips; a rare probe error falls
    // through to the read, preserving the old data-preserving behavior. (The
    // tradeoff: a producer that hasn't written its first byte by the time
    // roba probes is treated as no-context -- acceptable next to a hang.)
    if let Ok(false) = crate::stdin_probe::stdin_is_readable() {
        return Ok(None);
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(stdin_as_context(&buf))
}

/// Decide whether raw piped stdin should become a context part: the
/// trimmed content if non-empty, else `None`. Empty or whitespace-only
/// stdin is a no-op so a closed/empty pipe is byte-identical to no pipe.
fn stdin_as_context(raw: &str) -> Option<String> {
    let trimmed = raw.trim_end();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Assemble the final prompt from all sources, joined by blank lines.
/// Order: prepend files, piped stdin, attachments / git context, main,
/// append files. Piped stdin sits in the `--prepend`-like slot (it is
/// piped context, ahead of attachments and the main question).
///
/// Returns `Ok(None)` when nothing composed to a non-empty body (no
/// main prompt and no non-empty prepend/stdin/attach/append). The caller
/// decides what an empty composition means: on a TTY it guides the
/// user, off a TTY it errors. `Ok(Some(body))` carries the joined
/// prompt otherwise.
pub fn compose_prompt(
    main: Option<String>,
    prepend: &[PathBuf],
    piped_context: Option<String>,
    attachments: Option<String>,
    append: &[PathBuf],
) -> Result<Option<String>> {
    let mut parts: Vec<String> = Vec::new();
    for path in prepend {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading --prepend {}", path.display()))?;
        parts.push(content.trim_end().to_string());
    }
    if let Some(stdin_block) = piped_context {
        parts.push(stdin_block);
    }
    if let Some(attach_block) = attachments {
        parts.push(attach_block);
    }
    if let Some(m) = main {
        parts.push(m);
    }
    for path in append {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading --append {}", path.display()))?;
        parts.push(content.trim_end().to_string());
    }
    parts.retain(|p| !p.is_empty());
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(parts.join("\n\n")))
}

/// Substitute `{{KEY}}` placeholders in `prompt` with their values.
pub fn apply_vars(mut prompt: String, vars: &[(String, String)]) -> String {
    for (k, v) in vars {
        let placeholder = format!("{{{{{k}}}}}");
        prompt = prompt.replace(&placeholder, v);
    }
    prompt
}

/// Scan a resolved prompt for surviving identifier-shaped `{{NAME}}`
/// placeholders -- ones [`apply_vars`] did not substitute, usually a typo'd
/// `--var` key (`--var NMAE=x` leaves `{{NAME}}`). Returns each distinct
/// surviving placeholder in first-seen order (e.g. `{{NAME}}`).
///
/// Only `{{` + an identifier (`[A-Za-z_][A-Za-z0-9_]*`) + `}}` matches, so
/// legitimate literal prose braces (`{{ 1 + 2 }}`, `{{}}`, `{{ not-ident }}`)
/// do not false-fire. Hand-scanned to avoid a regex dependency.
pub fn find_unsubstituted_placeholders(prompt: &str) -> Vec<String> {
    let bytes = prompt.as_bytes();
    let mut found: Vec<String> = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let name_start = i + 2;
            let mut j = name_start;
            // First char of an identifier: letter or underscore.
            if j < bytes.len() && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'_') {
                j += 1;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                // Require a closing `}}` immediately after the identifier.
                if j + 1 < bytes.len() && bytes[j] == b'}' && bytes[j + 1] == b'}' {
                    let placeholder = format!("{{{{{}}}}}", &prompt[name_start..j]);
                    if !found.contains(&placeholder) {
                        found.push(placeholder);
                    }
                    i = j + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    found
}

/// Emit one stderr warning when a resolved prompt still carries
/// identifier-shaped `{{VAR}}` placeholders after substitution. The prompt
/// still sends (warn, not error -- legitimate literal braces can exist); this
/// just surfaces a likely typo'd `--var` key before it ships to claude.
pub fn warn_unsubstituted_placeholders(prompt: &str) {
    let leftover = find_unsubstituted_placeholders(prompt);
    if !leftover.is_empty() {
        eprintln!(
            "warning: unsubstituted placeholder(s): {} -- did you mean a different --var key?",
            leftover.join(", ")
        );
    }
}

/// Merge two optional strings with a blank line in between.
pub fn merge_optional(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (Some(s), None) | (None, Some(s)) => Some(s),
        (None, None) => None,
    }
}

/// Walk every `--attach` glob, fence each matched file, and return
/// one combined block. Patterns with no matches log a stderr warning
/// and otherwise continue.
pub fn collect_attachments(patterns: &[String]) -> Result<Option<String>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut blocks: Vec<String> = Vec::new();
    for pat in patterns {
        let matches = glob::glob(pat).with_context(|| format!("invalid glob: {pat}"))?;
        let mut had_any = false;
        for entry in matches {
            let path = entry.with_context(|| format!("walking --attach {pat}"))?;
            if !path.is_file() {
                continue;
            }
            had_any = true;
            check_attach_size(&path)?;
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("reading --attach {}", path.display()))?;
            blocks.push(format!(
                "File: {}\n```\n{}\n```",
                path.display(),
                content.trim_end()
            ));
        }
        if !had_any {
            eprintln!("warning: --attach {pat} matched no files");
        }
    }
    if blocks.is_empty() {
        Ok(None)
    } else {
        Ok(Some(blocks.join("\n\n")))
    }
}

/// Per-file size cap for `--attach`. A glob can fan out across a large tree
/// and silently catch a multi-GB build artifact or log; reading that into
/// memory before claude even starts would OOM. 10 MiB is generous for source
/// and config (the intended attach targets) while still catching a runaway
/// match. Fixed for now -- a `--max-attach-size` knob can come later if a
/// real need appears. Applies ONLY to the `--attach` glob path; `-f` /
/// `--prepend` / `--append` are single files the user named deliberately and
/// stay uncapped.
const MAX_ATTACH_BYTES: u64 = 10 * 1024 * 1024;

/// Error loudly if an attachment exceeds [`MAX_ATTACH_BYTES`], naming the file
/// and its size, rather than OOMing on the read or silently skipping it
/// (silent skip is its own data-loss bug). Stats the file without reading it.
fn check_attach_size(path: &Path) -> Result<()> {
    let len = std::fs::metadata(path)
        .with_context(|| format!("stat --attach {}", path.display()))?
        .len();
    if len > MAX_ATTACH_BYTES {
        bail!(
            "attachment \"{}\" is {}, exceeds the {} cap; narrow the --attach glob",
            path.display(),
            human_bytes(len),
            human_bytes(MAX_ATTACH_BYTES)
        );
    }
    Ok(())
}

/// Format a byte count as a short human-readable size (e.g. `2.3 GiB`,
/// `512 B`). Binary units to match the MiB cap.
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Build a git-context block from the diff / log / status flags.
pub fn collect_git_context(args: &AskArgs) -> Result<Option<String>> {
    let mut blocks: Vec<String> = Vec::new();
    if args.git_diff
        && let Some(out) = run_git(&["diff"])?
    {
        blocks.push(format!("git diff:\n```diff\n{out}\n```"));
    }
    if let Some(n) = args.git_log
        && let Some(out) = run_git(&["log", "-n", &n.to_string(), "--oneline"])?
    {
        blocks.push(format!("git log -n {n}:\n```\n{out}\n```"));
    }
    if args.git_status
        && let Some(out) = run_git(&["status", "--short"])?
    {
        blocks.push(format!("git status:\n```\n{out}\n```"));
    }
    if blocks.is_empty() {
        Ok(None)
    } else {
        Ok(Some(blocks.join("\n\n")))
    }
}

/// Run `git <args>` and return its trimmed stdout. `None` for empty
/// output (common when there's nothing to diff). Bubbles up non-zero
/// exits with the stderr message.
pub fn run_git(args: &[&str]) -> Result<Option<String>> {
    let output = std::process::Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        Ok(None)
    } else {
        Ok(Some(stdout))
    }
}

/// Read every byte from stdin into a trimmed String. Errors on empty
/// input so a stray pipe doesn't ship an empty prompt to claude.
pub fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let trimmed = buf.trim_end().to_string();
    if trimmed.is_empty() {
        bail!("empty stdin");
    }
    Ok(trimmed)
}

/// The scissors line that separates the user's prompt area (above)
/// from the reference block (below). Uses `//` prefix so it reads as
/// a code-style comment in editors that highlight markdown -- a
/// `#`-prefixed scissors would render as a heading in `.md` mode.
const SCISSORS: &str = "// ------------------------ >8 ------------------------";

/// Open `$VISUAL` / `$EDITOR` / `vi` on a `.md` scratch file. With
/// `history_n > 0` and a recent session in cwd, the file is
/// pre-filled in `git commit`-style layout: empty cursor area at the
/// top, scissors line, then the last N responses below as a
/// reference block. On save, everything from the scissors down is
/// stripped, so claude only sees what the user typed above.
///
/// `history_n == 0` (or "no last session in cwd") gives an empty
/// editor, same as the original behavior.
pub fn compose_in_editor(history_n: usize) -> Result<String> {
    let tmp = tempfile::Builder::new()
        .prefix("roba-prompt-")
        .suffix(".md")
        .tempfile()
        .context("creating editor scratch file")?;
    let path = tmp.path().to_path_buf();

    let preamble = if history_n == 0 {
        String::new()
    } else {
        let responses =
            crate::history::last_n_assistant_texts_in_cwd(history_n).unwrap_or_default();
        build_editor_preamble(&responses)
    };
    if !preamble.is_empty() {
        std::fs::write(&path, &preamble).context("writing editor preamble")?;
    }

    let editor = editor_command();
    let status =
        spawn_editor(&editor, &path).with_context(|| format!("running editor `{editor}`"))?;
    if !status.success() {
        bail!("editor exited with {status}");
    }
    let content = std::fs::read_to_string(&path).context("reading editor buffer")?;
    let body = strip_from_scissors(&content);
    let trimmed = body.trim().to_string();
    if trimmed.is_empty() {
        bail!("editor returned an empty prompt");
    }
    Ok(trimmed)
}

/// Build the preamble: empty cursor area at the top, scissors line
/// with `//`-prefixed instructions, then the reference block. The
/// response body itself is *unprefixed* plain text so it's visually
/// distinct from the `//` boilerplate. Returns empty if there are no
/// responses to show.
pub fn build_editor_preamble(responses: &[String]) -> String {
    if responses.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    // Two blank lines for the cursor area. nvim opens on line 1 by
    // default; the user types there, the scissors stays put below.
    out.push('\n');
    out.push('\n');
    out.push_str(SCISSORS);
    out.push('\n');
    out.push_str("// Reference only -- everything from the scissors down is stripped on save.\n");
    out.push_str("// Type your prompt above the scissors line.\n");
    out.push('\n');
    if responses.len() == 1 {
        out.push_str("// --- last response from the most recent session in this dir ---\n");
        out.push('\n');
        out.push_str(responses[0].trim_end());
        out.push('\n');
    } else {
        out.push_str(&format!(
            "// --- last {} responses from the most recent session in this dir (oldest first) ---\n",
            responses.len()
        ));
        for (i, r) in responses.iter().enumerate() {
            out.push('\n');
            out.push_str(&format!("// --- {} of {} ---\n", i + 1, responses.len()));
            out.push('\n');
            out.push_str(r.trim_end());
            out.push('\n');
        }
    }
    out
}

/// Return everything before the scissors line; if no scissors line
/// is found, return the whole content (defensive: a user who
/// deletes the scissors still gets their content sent, not silently
/// lost).
pub fn strip_from_scissors(content: &str) -> String {
    // Find the FIRST scissors line: the user's prompt area is above,
    // so an early match wins.
    let mut idx: Option<usize> = None;
    for (i, line) in content.lines().enumerate() {
        if line == SCISSORS {
            idx = Some(i);
            break;
        }
    }
    let Some(scissors_idx) = idx else {
        return content.to_string();
    };
    content
        .lines()
        .take(scissors_idx)
        .collect::<Vec<_>>()
        .join("\n")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_vars_substitutes_named_placeholders() {
        let prompt = "Hello {{NAME}}, ticket {{ID}}".to_string();
        let vars = vec![
            ("NAME".to_string(), "Josh".to_string()),
            ("ID".to_string(), "ABC-123".to_string()),
        ];
        assert_eq!(apply_vars(prompt, &vars), "Hello Josh, ticket ABC-123");
    }

    #[test]
    fn apply_vars_leaves_unknown_placeholders_alone() {
        let prompt = "{{KNOWN}} and {{UNKNOWN}}".to_string();
        let vars = vec![("KNOWN".to_string(), "yes".to_string())];
        assert_eq!(apply_vars(prompt, &vars), "yes and {{UNKNOWN}}");
    }

    #[test]
    fn apply_vars_handles_repeated_placeholders() {
        let prompt = "{{X}} and {{X}} again".to_string();
        let vars = vec![("X".to_string(), "go".to_string())];
        assert_eq!(apply_vars(prompt, &vars), "go and go again");
    }

    // -- unsubstituted placeholder detection (#286) ------------------------

    #[test]
    fn detects_surviving_placeholder() {
        let leftover = find_unsubstituted_placeholders("hi {{NAME}}");
        assert_eq!(leftover, vec!["{{NAME}}".to_string()]);
    }

    #[test]
    fn detects_multiple_distinct_placeholders_in_order() {
        let leftover = find_unsubstituted_placeholders("{{NAME}} did {{TICKET}}");
        assert_eq!(
            leftover,
            vec!["{{NAME}}".to_string(), "{{TICKET}}".to_string()]
        );
    }

    #[test]
    fn dedupes_repeated_placeholder() {
        let leftover = find_unsubstituted_placeholders("{{X}} and {{X}}");
        assert_eq!(leftover, vec!["{{X}}".to_string()]);
    }

    #[test]
    fn fully_substituted_prompt_has_no_leftovers() {
        // What `apply_vars` produces on a clean run: nothing to warn about.
        let resolved = apply_vars(
            "Hello {{NAME}}".to_string(),
            &[("NAME".to_string(), "Josh".to_string())],
        );
        assert!(find_unsubstituted_placeholders(&resolved).is_empty());
    }

    #[test]
    fn non_identifier_braces_do_not_false_fire() {
        // Literal prose / non-identifier braces must not be flagged.
        assert!(find_unsubstituted_placeholders("compute {{ 1 + 2 }}").is_empty());
        assert!(find_unsubstituted_placeholders("empty {{}} here").is_empty());
        assert!(find_unsubstituted_placeholders("{{not-an-ident}}").is_empty());
        assert!(find_unsubstituted_placeholders("a single { brace").is_empty());
    }

    #[test]
    fn underscore_and_digit_identifiers_match() {
        let leftover = find_unsubstituted_placeholders("{{_PRIVATE}} {{VAR2}}");
        assert_eq!(
            leftover,
            vec!["{{_PRIVATE}}".to_string(), "{{VAR2}}".to_string()]
        );
    }

    // -- attach size guard (#271) ------------------------------------------

    #[test]
    fn human_bytes_formats_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(10 * 1024 * 1024), "10.0 MiB");
        assert_eq!(human_bytes(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    #[test]
    fn under_cap_attachment_passes_size_check() {
        let f = write_temp("small file contents");
        assert!(check_attach_size(f.path()).is_ok());
    }

    #[test]
    fn over_cap_attachment_errors_with_name_and_size() {
        // Sparse file: set_len reports a logical size over the cap without
        // writing bytes, so the stat-first guard bails before any read.
        let f = tempfile::NamedTempFile::new().unwrap();
        f.as_file().set_len(MAX_ATTACH_BYTES + 1).unwrap();
        let err = check_attach_size(f.path()).unwrap_err().to_string();
        assert!(err.contains("exceeds the 10.0 MiB cap"), "got: {err}");
        assert!(
            err.contains(&f.path().display().to_string()),
            "error should name the file, got: {err}"
        );
    }

    #[test]
    fn merge_optional_combines_with_blank_line() {
        assert_eq!(
            merge_optional(Some("a".to_string()), Some("b".to_string())),
            Some("a\n\nb".to_string())
        );
    }

    #[test]
    fn merge_optional_returns_either_when_other_is_none() {
        assert_eq!(
            merge_optional(Some("a".to_string()), None),
            Some("a".to_string())
        );
        assert_eq!(
            merge_optional(None, Some("b".to_string())),
            Some("b".to_string())
        );
    }

    #[test]
    fn merge_optional_returns_none_when_both_none() {
        assert_eq!(merge_optional(None, None), None);
    }

    fn write_temp(content: &str) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "{content}").unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn compose_prompt_just_main() {
        let out = compose_prompt(Some("hi".to_string()), &[], None, None, &[]).unwrap();
        assert_eq!(out, Some("hi".to_string()));
    }

    #[test]
    fn compose_prompt_prepend_then_main_then_append() {
        let pre = write_temp("SYSTEM");
        let post = write_temp("CONTEXT");
        let out = compose_prompt(
            Some("question".to_string()),
            std::slice::from_ref(&pre.path().to_path_buf()),
            None,
            None,
            std::slice::from_ref(&post.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(out, Some("SYSTEM\n\nquestion\n\nCONTEXT".to_string()));
    }

    #[test]
    fn compose_prompt_inserts_attachments_between_prepend_and_main() {
        let pre = write_temp("PREP");
        let attach = "File: foo.rs\n```\nfn x() {}\n```".to_string();
        let out = compose_prompt(
            Some("question".to_string()),
            std::slice::from_ref(&pre.path().to_path_buf()),
            None,
            Some(attach.clone()),
            &[],
        )
        .unwrap();
        assert_eq!(out, Some(format!("PREP\n\n{attach}\n\nquestion")));
    }

    #[test]
    fn compose_prompt_main_optional_when_prepend_present() {
        let pre = write_temp("STANDALONE");
        let out = compose_prompt(
            None,
            std::slice::from_ref(&pre.path().to_path_buf()),
            None,
            None,
            &[],
        )
        .unwrap();
        assert_eq!(out, Some("STANDALONE".to_string()));
    }

    #[test]
    fn compose_prompt_none_when_everything_empty() {
        // Nothing to compose -> `None`. The caller (run_ask) turns this
        // into a TTY blurb (exit 0) or a non-TTY error (non-zero exit).
        let out = compose_prompt(None, &[], None, None, &[]).unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn compose_prompt_drops_empty_segments() {
        let empty = write_temp("");
        let out = compose_prompt(
            Some("only".to_string()),
            std::slice::from_ref(&empty.path().to_path_buf()),
            None,
            None,
            &[],
        )
        .unwrap();
        assert_eq!(out, Some("only".to_string()));
    }

    // -- piped stdin as context (--prepend-like slot) ----------------------

    #[test]
    fn compose_prompt_piped_context_sits_before_main() {
        // The piped block lands AFTER prepend and BEFORE the main question.
        let out = compose_prompt(
            Some("what's wrong here?".to_string()),
            &[],
            Some("ERROR: boom".to_string()),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(out, Some("ERROR: boom\n\nwhat's wrong here?".to_string()));
    }

    #[test]
    fn compose_prompt_piped_context_order_prepend_stdin_attach_main() {
        // Pin the full slot order: prepend, piped stdin, attachments, main.
        let pre = write_temp("PREP");
        let attach = "File: foo.rs\n```\nfn x() {}\n```".to_string();
        let out = compose_prompt(
            Some("question".to_string()),
            std::slice::from_ref(&pre.path().to_path_buf()),
            Some("PIPED".to_string()),
            Some(attach.clone()),
            &[],
        )
        .unwrap();
        assert_eq!(out, Some(format!("PREP\n\nPIPED\n\n{attach}\n\nquestion")));
    }

    #[test]
    fn stdin_as_context_keeps_real_content() {
        assert_eq!(
            stdin_as_context("ERROR: boom\n"),
            Some("ERROR: boom".to_string())
        );
    }

    #[test]
    fn stdin_as_context_empty_is_none() {
        // The load-bearing guard: an empty pipe is byte-identical to no pipe.
        assert_eq!(stdin_as_context(""), None);
    }

    #[test]
    fn stdin_as_context_whitespace_only_is_none() {
        assert_eq!(stdin_as_context("   \n\t \n"), None);
    }

    #[test]
    fn no_prompt_blurb_contains_examples_and_help_pointer() {
        let blurb = crate::cli::no_prompt_blurb();
        // An example line from AFTER_HELP survives the single-sourcing.
        assert!(
            blurb.contains("one finite operation"),
            "expected an example line, got:\n{blurb}"
        );
        // The pointer to the full reference is present.
        assert!(
            blurb.contains("roba --help"),
            "expected a `roba --help` pointer, got:\n{blurb}"
        );
        // The header is single-sourced from the package description, so it
        // matches the `about` line shown by `-h`/`--help`.
        assert!(
            blurb.starts_with(env!("CARGO_PKG_DESCRIPTION")),
            "expected the blurb to lead with the package description, got:\n{blurb}"
        );
        // `No prompt given.` sits on its own line so it reads as the error.
        assert!(
            blurb.contains("\nNo prompt given.\n"),
            "expected `No prompt given.` on its own line, got:\n{blurb}"
        );
    }

    // -- editor preamble + scissors strip ----------------------------------

    #[test]
    fn preamble_empty_for_no_responses() {
        assert_eq!(build_editor_preamble(&[]), "");
    }

    #[test]
    fn preamble_single_response_layout() {
        let p = build_editor_preamble(&["the previous answer".to_string()]);
        // Starts with blank lines so the cursor lands above the scissors.
        assert!(
            p.starts_with("\n\n"),
            "expected leading blank lines for cursor area, got: {p:?}"
        );
        assert!(p.contains(SCISSORS));
        // Hint text is `//`-prefixed and visible.
        assert!(p.contains("// Type your prompt above the scissors line."));
        // Section divider for the response.
        assert!(p.contains("// --- last response"));
        // The response body itself is unprefixed -- visually distinct
        // from the `//` instructions/dividers.
        assert!(
            p.contains("\nthe previous answer\n"),
            "expected unprefixed response body, got:\n{p}"
        );
    }

    #[test]
    fn preamble_multi_response_uses_section_dividers() {
        let p = build_editor_preamble(&["older one".to_string(), "newer one".to_string()]);
        // Header mentions the count
        assert!(p.contains("// --- last 2 responses"));
        // Each response gets a numbered divider
        assert!(p.contains("// --- 1 of 2 ---"));
        assert!(p.contains("// --- 2 of 2 ---"));
        // Bodies are unprefixed
        assert!(p.contains("\nolder one\n"));
        assert!(p.contains("\nnewer one\n"));
        assert!(p.contains(SCISSORS));
    }

    #[test]
    fn preamble_preserves_blank_lines_in_response() {
        let p = build_editor_preamble(&["first\n\nsecond".to_string()]);
        // Response goes in verbatim (unprefixed), blank lines included.
        assert!(
            p.contains("\nfirst\n\nsecond\n"),
            "expected verbatim response with blank line preserved, got:\n{p}"
        );
    }

    #[test]
    fn strip_returns_content_above_scissors() {
        let buf = format!("my prompt line 1\nmy prompt line 2\n\n{SCISSORS}\n// reference");
        assert_eq!(
            strip_from_scissors(&buf),
            "my prompt line 1\nmy prompt line 2\n"
        );
    }

    #[test]
    fn strip_uses_first_scissors_when_multiple_exist() {
        // Defensive: the FIRST scissors wins (user's prompt is above).
        // A stray scissors inside reference content can't hijack the split.
        let buf = format!("real prompt\n{SCISSORS}\n// stuff\n{SCISSORS}\n// more");
        assert_eq!(strip_from_scissors(&buf), "real prompt");
    }

    #[test]
    fn strip_no_scissors_returns_whole_content() {
        // Defensive: if the user deletes the scissors, send what they
        // typed rather than losing it.
        let buf = "just the prompt\nno scissors here";
        assert_eq!(strip_from_scissors(buf), buf);
    }

    #[test]
    fn strip_preserves_markdown_headers_in_prompt() {
        // The whole reason for scissors-based strip over `#`-line
        // filtering: user's markdown headers in the prompt survive.
        let buf = format!("# Real heading\n## Sub\nbody\n{SCISSORS}\n// reference");
        assert_eq!(strip_from_scissors(&buf), "# Real heading\n## Sub\nbody");
    }
}
