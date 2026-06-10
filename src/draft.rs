//! Shared plumbing for the claude-assisted `draft`/`init` verbs
//! (`roba alias draft`, `roba profile draft`, `roba config init`).
//!
//! All three verbs use the same deterministic bookends: build a prompt
//! from the bundled, parse-tested config schema + the user's words, make
//! ONE lean claude call, then validate the output with roba's REAL
//! deserializer (`deny_unknown_fields`). The type-specific parts -- which
//! schema section, which deserializer, how to render -- live in the
//! calling module ([`crate::aliases`], [`crate::profile`],
//! [`crate::config`]). The generic parts -- the claude call,
//! fence-stripping, and block-appending -- live here, so there is one
//! shared core rather than parallel copies.

use std::path::Path;

use anyhow::{Context, Result};
use claude_wrapper::{Claude, QueryCommand};

/// Make the single lean generation call and return claude's raw result
/// text for the caller to validate.
///
/// PURE generation: no tools (the alias/profile drafts generate a block
/// from the schema + description alone), no session kept (a draft is not
/// a thread worth resuming), stdin-fed, optional model override. NOT
/// routed through `run_ask`.
pub async fn generate(prompt: String, model: Option<&str>, call_name: &str) -> Result<String> {
    run_generation(prompt, model, call_name, &[], None).await
}

/// Like [`generate`], but lets the call SEE the current project: a
/// read-only Read/Glob/Grep tool posture so claude can skim the README /
/// manifest / layout before drafting, bounded by a turn cap so a
/// bootstrap skims rather than spelunks. Used by `roba config init`,
/// which fits a whole-file config to what it observes in the cwd.
pub async fn generate_inspecting(
    prompt: String,
    model: Option<&str>,
    call_name: &str,
) -> Result<String> {
    run_generation(
        prompt,
        model,
        call_name,
        &["Read", "Glob", "Grep"],
        Some(20),
    )
    .await
}

/// Shared core for both generation postures. `allowed_tools` empty means
/// no tools (pure generation); a non-empty list opens a read-only window
/// onto the project. `max_turns` bounds an inspecting call.
async fn run_generation(
    prompt: String,
    model: Option<&str>,
    call_name: &str,
    allowed_tools: &[&str],
    max_turns: Option<u32>,
) -> Result<String> {
    let claude = Claude::builder().build()?;
    let mut cmd = QueryCommand::new(prompt)
        .name(call_name)
        .prompt_via_stdin(true)
        .no_session_persistence();
    if !allowed_tools.is_empty() {
        let tools: Vec<String> = allowed_tools.iter().map(|s| s.to_string()).collect();
        cmd = cmd.allowed_tools(tools);
    }
    if let Some(n) = max_turns {
        cmd = cmd.max_turns(n);
    }
    if let Some(model) = model {
        cmd = cmd.model(model.to_string());
    }
    let result = cmd.execute_json(&claude).await?;
    Ok(result.result)
}

/// Strip a single surrounding markdown code fence if present, so output
/// wrapped in ```toml ... ``` still parses. Fence-free input is returned
/// trimmed but otherwise untouched.
pub fn strip_code_fences(s: &str) -> String {
    let trimmed = s.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines: Vec<&str> = trimmed.lines().collect();
    lines.remove(0); // opening ``` or ```toml
    if lines
        .last()
        .is_some_and(|l| l.trim_start().starts_with("```"))
    {
        lines.pop(); // closing ```
    }
    lines.join("\n")
}

/// Append a blank line + the block to `path`, creating the file (and any
/// missing parent dirs) if absent.
pub fn append_block(path: &Path, block: &str) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent directory for {}", path.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {} for append", path.display()))?;
    write!(file, "\n{block}").with_context(|| format!("writing to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_code_fences_unwraps_toml_fence() {
        let raw = "```toml\n[alias.x]\ndescription = \"hi\"\n```";
        assert_eq!(strip_code_fences(raw), "[alias.x]\ndescription = \"hi\"");
    }

    #[test]
    fn strip_code_fences_leaves_plain_input() {
        let raw = "[alias.x]\ndescription = \"hi\"\n";
        assert_eq!(strip_code_fences(raw), "[alias.x]\ndescription = \"hi\"");
    }

    #[test]
    fn append_block_creates_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("roba.toml");
        append_block(&path, "[profile.x]\nreadonly = true\n").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "\n[profile.x]\nreadonly = true\n");
        // A second append stacks below the first with a blank-line gap.
        append_block(&path, "[profile.y]\nwritable = true\n").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[profile.x]") && text.contains("[profile.y]"),
            "{text}"
        );
    }
}
