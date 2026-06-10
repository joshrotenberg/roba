//! Shared plumbing for the claude-assisted `draft` verbs
//! (`roba alias draft`, `roba profile draft`).
//!
//! Both verbs use the same deterministic bookends: build a prompt from
//! the bundled, parse-tested config schema + the user's words, make ONE
//! lean claude call, then validate the output with roba's REAL
//! deserializer (`deny_unknown_fields`) and normalize it back to a
//! canonical block. The type-specific parts -- which schema section,
//! which deserializer, how to render -- live in the calling module
//! ([`crate::aliases`], [`crate::profile`]). The generic parts -- the
//! claude call, fence-stripping, and block-appending -- live here, so
//! there is one shared core rather than two parallel copies.

use std::path::Path;

use anyhow::{Context, Result};
use claude_wrapper::{Claude, QueryCommand};

/// Make the single lean generation call and return claude's raw result
/// text for the caller to validate.
///
/// Read-only default tool posture (generation needs no tools), no
/// session kept (a draft is not a thread worth resuming), stdin-fed,
/// optional model override. NOT routed through `run_ask`.
pub async fn generate(prompt: String, model: Option<&str>, call_name: &str) -> Result<String> {
    let claude = Claude::builder().build()?;
    let mut cmd = QueryCommand::new(prompt)
        .name(call_name)
        .prompt_via_stdin(true)
        .no_session_persistence();
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
