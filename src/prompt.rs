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

/// Resolve the main prompt body from one of: editor, file, positional
/// arg, stdin (piped or explicit `-`). Returns `Ok(None)` only when
/// no source was given and stdin is a TTY -- the caller should then
/// rely on prepend/append/attach to supply content, or error if those
/// are also empty.
pub fn resolve_main_prompt(
    positional: Option<&str>,
    file: Option<&Path>,
    editor: bool,
) -> Result<Option<String>> {
    if editor {
        if !std::io::stdin().is_terminal() {
            bail!("--editor requires a TTY; pipe-mode input is incompatible");
        }
        return Ok(Some(compose_in_editor()?));
    }
    if let Some(path) = file {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading prompt from {}", path.display()))?;
        let trimmed = content.trim_end().to_string();
        if trimmed.is_empty() {
            bail!("file {} is empty", path.display());
        }
        return Ok(Some(trimmed));
    }
    match positional {
        Some("-") => Ok(Some(read_stdin()?)),
        Some(p) => Ok(Some(p.to_string())),
        None => {
            if std::io::stdin().is_terminal() {
                Ok(None)
            } else {
                Ok(Some(read_stdin()?))
            }
        }
    }
}

/// Assemble the final prompt from all sources, joined by blank lines.
/// Order: prepend files, attachments / git context, main, append files.
pub fn compose_prompt(
    main: Option<String>,
    prepend: &[PathBuf],
    attachments: Option<String>,
    append: &[PathBuf],
) -> Result<String> {
    let mut parts: Vec<String> = Vec::new();
    for path in prepend {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading --prepend {}", path.display()))?;
        parts.push(content.trim_end().to_string());
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
        bail!(
            "no prompt: pass one as an argument, use -f / -e, --prepend / --append / --attach, pipe via stdin, or use `-` for stdin"
        );
    }
    Ok(parts.join("\n\n"))
}

/// Substitute `{{KEY}}` placeholders in `prompt` with their values.
pub fn apply_vars(mut prompt: String, vars: &[(String, String)]) -> String {
    for (k, v) in vars {
        let placeholder = format!("{{{{{k}}}}}");
        prompt = prompt.replace(&placeholder, v);
    }
    prompt
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

/// Open `$VISUAL` / `$EDITOR` / `vi` on an empty `.md` scratch file
/// and return whatever the user typed on save + exit.
pub fn compose_in_editor() -> Result<String> {
    let tmp = tempfile::Builder::new()
        .prefix("cwr-prompt-")
        .suffix(".md")
        .tempfile()
        .context("creating editor scratch file")?;
    let path = tmp.path().to_path_buf();
    let editor = editor_command();
    let status = spawn_editor(&editor, &path)
        .with_context(|| format!("running editor `{editor}`"))?;
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
}
