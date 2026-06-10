//! The `roba config init` subcommand: claude-assisted, parse-validated
//! per-project `roba.toml` bootstrap.
//!
//! The per-project sibling of `roba alias draft` / `roba profile draft`.
//! Those generate ONE block for your user config from the schema +
//! description alone; `init` instead drafts a WHOLE starter project file
//! fitted to the current project. Two deliberate differences from the
//! pure-generation siblings:
//!
//! 1. The generation call SEES the project -- a read-only Read/Glob/Grep
//!    posture (via [`crate::draft::generate_inspecting`]) so claude can
//!    skim the README / manifest / layout before drafting.
//! 2. The output is a complete file, so it is validated with the REAL
//!    per-file config deserializer ([`crate::profile::pool::parse_config_str`],
//!    `deny_unknown_fields` on every section), not a single-block wrapper.
//!
//! stdout is the validated file content ONLY (pipeable to `> roba.toml`);
//! everything else goes to stderr. No retry loop: an invalid draft fails
//! loud with the deserializer error plus the raw model output.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::cli::ConfigInitArgs;

/// Run `roba config init [DESCRIPTION] [--write [PATH]] [--model NAME]`.
pub async fn run_init(args: ConfigInitArgs) -> Result<()> {
    // 0. Fail fast on a clobber: a whole-file write must never overwrite,
    //    and the target path doesn't depend on the generated content, so
    //    check it BEFORE spending a (paid) claude call.
    if let Some(target) = &args.write {
        let path = write_target_path(target);
        if path.exists() {
            bail!("{}", clobber_msg(&path));
        }
    }

    // 1. Deterministic prompt: the whole bundled schema + optional steer.
    let prompt = init_prompt(args.description.as_deref());

    // 2. One lean claude call with a read-only window onto the project,
    //    bounded turns. NOT routed through `run_ask`.
    let raw = crate::draft::generate_inspecting(prompt, args.model.as_deref(), "roba: config init")
        .await?;

    // 3. Validate the WHOLE file through the real per-file deserializer.
    //    Comments are preserved (we print the cleaned text, not a
    //    re-serialization), so a starter file stays human-readable.
    let content = validate_config(&raw)?;

    // 4. Optional write -- never clobbering an existing config.
    if let Some(target) = &args.write {
        let path = write_no_clobber(target, &content)?;
        eprintln!("wrote {}", path.display());
        eprintln!(
            "this file joins the config pool (closer-to-cwd files win); `roba profile list` will now show the merged set"
        );
    }

    // stdout = the validated file only, byte-clean (pipeable to `> roba.toml`).
    print!("{content}");
    Ok(())
}

/// Build the generation prompt: the ENTIRE bundled, parse-tested sample
/// config (for a whole file the whole schema is the right grounding) +
/// drafting instructions + the user's steer when given.
fn init_prompt(description: Option<&str>) -> String {
    let sample = crate::profile::STARTER_CONFIG_TOML;
    let steering = match description {
        Some(d) => format!("\n\nThe user added this steer; weave it into the draft:\n{d}"),
        None => String::new(),
    };
    format!(
        "You are drafting a STARTER project `roba.toml` for the project in the \
         current directory.\n\n\
         roba is a CLI that sugars `claude -p`. A project `roba.toml` sets \
         top-level defaults plus named `[profile.NAME]` overlays and \
         `[alias.NAME]` shortcut verbs. Here is the complete, annotated config \
         schema -- every valid key appears and is commented. Use ONLY keys that \
         appear here; do not invent fields:\n\n\
         {sample}\n\n\
         First, briefly inspect THIS project with the Read/Glob/Grep tools (the \
         README, the package manifest, the top-level layout) so the draft fits \
         what you see. Skim, do not spelunk.\n\n\
         Then produce a starter project roba.toml that:\n\
         - Opens with conservative top-level defaults appropriate to this project.\n\
         - Defines a couple of useful `[profile.NAME]` overlays fitted to the project.\n\
         - Defines AT MOST one or two `[alias.NAME]` shortcut verbs, and only if clearly useful.\n\
         - Keeps every profile/permission posture READ-ONLY unless the steer below asks otherwise.\n\
         - Has a brief `#` comment on every key explaining what it does.\n\
         - Does NOT include a `[session]` table (session UUIDs are machine-local).\n\
         - Uses ONLY keys from the schema above; the whole file must parse as valid TOML.\
         {steering}\n\n\
         Output requirements (follow exactly):\n\
         - Output the roba.toml file content and NOTHING else.\n\
         - Do NOT wrap the output in markdown code fences.\n\
         - Do NOT include any prose or explanation outside TOML `#` comments."
    )
}

/// Extract the config body from a raw model response, then validate the
/// WHOLE file through roba's real per-file config deserializer.
///
/// Extraction tolerates the two output artifacts a model commonly emits
/// despite instructions: a surrounding ```` ```toml ```` fence, and a
/// leading conversational preamble ("Here's the starter roba.toml:")
/// before the config. Whole-file output is noticeably more preamble-prone
/// than the single-block draft verbs, so this normalization is what makes
/// stdout reliably pipeable. It only ever DROPS leading non-config text;
/// the surviving body still runs through the real deserializer, so a
/// genuinely malformed draft still fails loud with the deserializer error
/// plus the raw model output.
fn validate_config(raw: &str) -> Result<String> {
    let cleaned = extract_config_toml(raw);
    crate::profile::pool::parse_config_str(&cleaned).map_err(|e| {
        anyhow::anyhow!("drafted config did not parse: {e:#}\n\n--- raw model output ---\n{raw}")
    })?;
    Ok(cleaned)
}

/// Pull the TOML body out of `raw`: prefer a fenced code block if one is
/// present (the model wrapped the file), otherwise drop any leading prose
/// preamble (skip lines until the first that looks like TOML). Returns
/// the remainder verbatim so comments survive.
fn extract_config_toml(raw: &str) -> String {
    let trimmed = raw.trim();
    let body = fenced_block(trimmed).unwrap_or_else(|| trimmed.to_string());
    strip_leading_preamble(&body)
}

/// If `s` contains a ```` ``` ````-fenced block, return its inner content
/// (language tag and fences removed). The first fence opens the block and
/// the next closes it. `None` when there is no closing fence.
fn fenced_block(s: &str) -> Option<String> {
    let open = s.find("```")?;
    let after_open = &s[open + 3..];
    // Skip the rest of the opening fence line (an optional language tag).
    let nl = after_open.find('\n')?;
    let body_start = open + 3 + nl + 1;
    let close_rel = s[body_start..].find("```")?;
    Some(s[body_start..body_start + close_rel].trim_end().to_string())
}

/// Drop leading lines until the first that looks like TOML, removing a
/// conversational preamble. If no line looks like TOML, return the input
/// unchanged so the deserializer fails loud rather than silently emitting
/// nothing.
fn strip_leading_preamble(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    match lines.iter().position(|l| looks_like_toml_line(l)) {
        Some(i) => lines[i..].join("\n"),
        None => s.to_string(),
    }
}

/// A line that plausibly begins TOML content: a comment, a table header,
/// or a `key = value` assignment with a bare key. Prose preamble lines
/// (which have spaces in the part before any `=`, or no `=` at all) do
/// not match.
fn looks_like_toml_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    if t.starts_with('#') || t.starts_with('[') {
        return true;
    }
    match t.split_once('=') {
        Some((key, _)) => {
            let key = key.trim();
            !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '"'))
        }
        None => false,
    }
}

/// Resolve the `--write` target: bare `--write` => `./roba.toml` (the
/// project file -- this is the per-project verb), `--write PATH` => PATH.
fn write_target_path(target: &Option<PathBuf>) -> PathBuf {
    target.clone().unwrap_or_else(|| PathBuf::from("roba.toml"))
}

/// The clobber-refusal message, shared by the fail-fast pre-check and the
/// race-safe re-check in [`write_no_clobber`].
fn clobber_msg(path: &std::path::Path) -> String {
    format!(
        "{} already exists; refusing to overwrite a whole-file config. Print to stdout and merge into it by hand instead.",
        path.display()
    )
}

/// Write `content` to the resolved `--write` target, REFUSING to
/// overwrite an existing file. A whole-file verb must never clobber or
/// append to an existing config. The fail-fast pre-check in [`run_init`]
/// already covers the common case; this re-check closes the
/// generate-then-write race.
fn write_no_clobber(target: &Option<PathBuf>, content: &str) -> Result<PathBuf> {
    let path = write_target_path(target);
    if path.exists() {
        bail!("{}", clobber_msg(&path));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent directory for {}", path.display()))?;
    }
    std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_config_accepts_the_bundled_sample() {
        // The bundled, parse-tested sample IS a known-good whole file --
        // it must pass the same validator config init uses.
        let content = validate_config(crate::profile::STARTER_CONFIG_TOML).unwrap();
        assert!(content.contains("[profile.review]"), "{content}");
    }

    #[test]
    fn validate_config_strips_fences_first() {
        let raw = "```toml\nreadonly = true\n\n[profile.x]\ngit_diff = true\n```";
        let content = validate_config(raw).unwrap();
        assert!(content.starts_with("readonly = true"), "{content}");
        assert!(!content.contains("```"), "{content}");
    }

    #[test]
    fn validate_config_drops_leading_prose_preamble() {
        // The real haiku failure mode: a conversational preamble before
        // the config. Extraction drops it; the rest validates and survives
        // verbatim (comments intact).
        let raw = "Perfect. Here's the starter `roba.toml`:\n\n\
                   # top-level defaults\n\
                   readonly = true\n\n\
                   [profile.review]\n\
                   git_diff = true\n";
        let content = validate_config(raw).unwrap();
        assert!(content.starts_with("# top-level defaults"), "{content}");
        assert!(!content.contains("Perfect."), "{content}");
        assert!(content.contains("[profile.review]"), "{content}");
    }

    #[test]
    fn validate_config_handles_preamble_then_fence() {
        let raw = "Here is the file:\n```toml\nreadonly = true\n```";
        let content = validate_config(raw).unwrap();
        assert_eq!(content, "readonly = true");
    }

    #[test]
    fn validate_config_rejects_unknown_top_level_key() {
        let raw = "made_up_top_level = true\n";
        let err = validate_config(raw).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("did not parse"), "{msg}");
        assert!(msg.contains("made_up_top_level"), "{msg}");
        assert!(msg.contains("raw model output"), "{msg}");
    }

    #[test]
    fn validate_config_rejects_unknown_profile_key() {
        let raw = "[profile.x]\nbogus_profile_key = true\n";
        let err = validate_config(raw).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("did not parse"), "{msg}");
        assert!(msg.contains("bogus_profile_key"), "{msg}");
    }

    #[test]
    fn write_no_clobber_refuses_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roba.toml");
        std::fs::write(&path, "readonly = true\n").unwrap();
        let err = write_no_clobber(&Some(path.clone()), "writable = true\n").unwrap_err();
        assert!(format!("{err:#}").contains("already exists"), "{err:#}");
        // The existing file is untouched.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "readonly = true\n");
    }

    #[test]
    fn write_no_clobber_writes_a_fresh_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("roba.toml");
        let written = write_no_clobber(&Some(path.clone()), "readonly = true\n").unwrap();
        assert_eq!(written, path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "readonly = true\n");
    }

    #[test]
    fn init_prompt_includes_steering_and_schema() {
        let p = init_prompt(Some("focus on PR review and a docs-build verb"));
        assert!(
            p.contains("focus on PR review and a docs-build verb"),
            "{p}"
        );
        // The whole schema is embedded for grounding.
        assert!(p.contains("[profile.review]"), "schema missing from prompt");
        // The no-session instruction is present.
        assert!(p.contains("[session]"), "{p}");
    }

    #[test]
    fn init_prompt_without_steering_is_clean() {
        let p = init_prompt(None);
        assert!(p.contains("STARTER project"), "{p}");
        // No dangling "steer" sentence when none was given.
        assert!(!p.contains("The user added this steer"), "{p}");
    }
}
