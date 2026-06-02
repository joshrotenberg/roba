//! Shared install / list / show logic for the bundled skill and agent
//! libraries.
//!
//! The skill and agent bundles have identical shape -- a directory per
//! item, each holding a primary doc (`SKILL.md` / `AGENT.md`) with YAML
//! frontmatter plus optional extra files. Rather than duplicate the
//! three subcommand handlers, [`crate::skills`] and [`crate::agents`]
//! embed their generated `&[(name, relative_path, body)]` array and
//! delegate here, passing a [`Kind`] that names the destination
//! directory and primary doc filename.

use anyhow::{Context, Result, bail};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

/// Canonical base for the rendered mdbook site (humans). The GitHub
/// Pages URL 301-redirects to any configured custom domain, so this
/// stays the stable, repo-pinned canonical form.
pub const DOCS_RENDERED_BASE: &str = "https://joshrotenberg.github.io/roba";

/// Canonical base for the raw markdown source on GitHub. Agents can
/// `WebFetch` this directly -- no HTML parsing needed.
pub const DOCS_RAW_BASE: &str = "https://raw.githubusercontent.com/joshrotenberg/roba/main";

/// Rendered (mdbook) doc URL for one bundled item. The aggregator
/// flattens `<dir>/<name>/<doc>` to a single `<dir>/<name>.md` source
/// page, which mdbook serves at `<dir>/<name>.html`.
pub fn rendered_url(kind: Kind, name: &str) -> String {
    format!("{DOCS_RENDERED_BASE}/{}/{name}.html", kind.dir)
}

/// Raw markdown source URL for one bundled item: the in-repo
/// `<dir>/<name>/<doc>` path under the raw-content base.
pub fn raw_url(kind: Kind, name: &str) -> String {
    format!("{DOCS_RAW_BASE}/{}/{name}/{}", kind.dir, kind.doc)
}

/// Rendered doc URL for a skill by name.
pub fn skill_rendered_url(name: &str) -> String {
    rendered_url(SKILLS, name)
}

/// Raw `SKILL.md` source URL for a skill by name.
pub fn skill_raw_url(name: &str) -> String {
    raw_url(SKILLS, name)
}

/// Rendered doc URL for an agent by name.
pub fn agent_rendered_url(name: &str) -> String {
    rendered_url(AGENTS, name)
}

/// Raw `AGENT.md` source URL for an agent by name.
pub fn agent_raw_url(name: &str) -> String {
    raw_url(AGENTS, name)
}

/// One bundled file: `(name, relative_path, body)`.
///
/// - `name` -- the item's directory name (e.g. `"draft-pr-first"`), or
///   the file stem for files sitting directly under the bundle root
///   (e.g. `"README"` for `skills/README.md`).
/// - `relative_path` -- path relative to the bundle root, forward-slash
///   separated (e.g. `"draft-pr-first/SKILL.md"`).
/// - `body` -- the file content, embedded via `include_str!`.
pub type Entry = (&'static str, &'static str, &'static str);

/// Which library is being operated on. Carries the per-library
/// specifics: the install destination subdirectory and the primary doc
/// filename that `list` / `show` read frontmatter and bodies from.
#[derive(Clone, Copy)]
pub struct Kind {
    /// Subdirectory under `~/.claude/` (`"skills"` or `"agents"`).
    pub dir: &'static str,
    /// Singular noun for messages (`"skill"` / `"agent"`).
    pub noun: &'static str,
    /// Primary doc filename (`"SKILL.md"` / `"AGENT.md"`).
    pub doc: &'static str,
}

pub const SKILLS: Kind = Kind {
    dir: "skills",
    noun: "skill",
    doc: "SKILL.md",
};

pub const AGENTS: Kind = Kind {
    dir: "agents",
    noun: "agent",
    doc: "AGENT.md",
};

/// True for entries that represent per-item content (those nested under
/// an item directory, i.e. whose relative path has a `/`). Top-level
/// files like `skills/README.md` are repo documentation, not per-item
/// content, so they are excluded from installation.
fn is_installable(entry: &Entry) -> bool {
    entry.1.contains('/')
}

/// `~/.claude/<kind.dir>/`. Errors if the home directory can't be
/// resolved.
fn default_dest(kind: Kind) -> Result<PathBuf> {
    let home = crate::profile::home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory (HOME unset)"))?;
    Ok(home.join(".claude").join(kind.dir))
}

/// `roba {skill,agent} install` runner.
pub fn run_install(bundle: &[Entry], args: crate::cli::InstallArgs, kind: Kind) -> Result<()> {
    let dest = match args.to {
        Some(p) => p,
        None => default_dest(kind)?,
    };
    let interactive = std::io::stdin().is_terminal();
    let mut wrote = 0usize;
    let mut skipped = 0usize;

    for entry in bundle.iter().filter(|e| is_installable(e)) {
        let (_, rel, body) = entry;
        let target = dest.join(rel);

        if target.exists() {
            let overwrite = if args.force {
                true
            } else if args.skip {
                false
            } else {
                // Default: ask, defaulting to skip. With no TTY we
                // can't ask, so we skip (the safe choice -- never
                // clobber without consent).
                prompt_overwrite(&target, interactive)?
            };
            if !overwrite {
                eprintln!("skipped {} (exists)", target.display());
                skipped += 1;
                continue;
            }
        }

        if args.dry_run {
            eprintln!("would write {}", target.display());
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&target, body)
                .with_context(|| format!("writing {}", target.display()))?;
            eprintln!("wrote {}", target.display());
        }
        wrote += 1;
    }

    if args.dry_run {
        eprintln!(
            "dry run: would install {wrote} file(s) to {} ({skipped} skipped)",
            dest.display()
        );
    } else {
        eprintln!(
            "installed {wrote} {} file(s) to {} ({skipped} skipped)",
            kind.noun,
            dest.display()
        );
    }
    Ok(())
}

/// Prompt the user to overwrite an existing file. Returns the chosen
/// action; defaults to "skip" (false) on a bare Enter or when stdin
/// isn't interactive (we can't ask, so we don't clobber).
fn prompt_overwrite(target: &Path, interactive: bool) -> Result<bool> {
    if !interactive {
        return Ok(false);
    }
    eprint!("{} exists -- overwrite? [y/N] ", target.display());
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading overwrite confirmation")?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// `roba {skill,agent} list` runner: one row per item with its
/// frontmatter `description`. When `urls` is set, the description is
/// replaced by the rendered + raw doc URLs (the canonical URLs are
/// keyed off the item's directory name, not its frontmatter `name`).
pub fn run_list(bundle: &[Entry], kind: Kind, urls: bool) -> Result<()> {
    // Each row: (display_name, description, dir_name). The dir name is
    // the URL key; the display name (frontmatter `name`) is cosmetic.
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for (name, rel, body) in bundle.iter().filter(|e| is_installable(e)) {
        if !rel.ends_with(kind.doc) {
            continue;
        }
        let fm = parse_frontmatter(body);
        let display_name = fm.get("name").cloned().unwrap_or_else(|| name.to_string());
        let description = fm.get("description").cloned().unwrap_or_default();
        rows.push((display_name, description, name.to_string()));
    }
    rows.sort();
    if rows.is_empty() {
        eprintln!("no bundled {}s", kind.noun);
        return Ok(());
    }
    let width = rows.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
    for (name, desc, dir_name) in rows {
        if urls {
            // URL columns instead of the description: name, rendered, raw.
            println!(
                "{name:<width$}  {}  {}",
                rendered_url(kind, &dir_name),
                raw_url(kind, &dir_name)
            );
        } else if desc.is_empty() {
            println!("{name}");
        } else {
            println!("{name:<width$}  {desc}");
        }
    }
    Ok(())
}

/// `roba {skill,agent} show NAME` runner: print the primary doc body
/// verbatim (frontmatter included). Errors if no item matches.
///
/// When `url` is set, print only the canonical doc URLs (rendered +
/// raw), one per line, and skip the body -- the caller wanted the
/// URLs, not the content. URLs are keyed off the item's directory
/// name (the URL path component), not the matched query string.
pub fn run_show(bundle: &[Entry], name: &str, kind: Kind, url: bool) -> Result<()> {
    for (item_name, rel, body) in bundle.iter().filter(|e| is_installable(e)) {
        if !rel.ends_with(kind.doc) {
            continue;
        }
        // Match either the directory name or the frontmatter `name`.
        let fm_name = parse_frontmatter(body).get("name").cloned();
        if *item_name == name || fm_name.as_deref() == Some(name) {
            if url {
                println!("rendered: {}", rendered_url(kind, item_name));
                println!("raw:      {}", raw_url(kind, item_name));
            } else {
                print!("{body}");
            }
            return Ok(());
        }
    }
    bail!("no bundled {} named `{name}`", kind.noun);
}

/// Minimal YAML-frontmatter parser: pulls top-level `key: value` pairs
/// from the `---`-delimited block at the head of a markdown file.
///
/// Deliberately hand-rolled instead of pulling in `serde_yaml`: the
/// only consumers are `list` (needs `name` + `description`) and `show`
/// (needs `name`), both of which want a flat string map of the leading
/// scalar fields. A full YAML parser would be far heavier than the job.
/// Splits each line on the first `:`; ignores nested / multi-line
/// values, which the bundled docs don't use for these fields.
fn parse_frontmatter(body: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut lines = body.lines();
    // Frontmatter must open with `---` on the first line.
    if lines.next().map(str::trim) != Some("---") {
        return map;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            if key.is_empty() {
                continue;
            }
            map.insert(key.to_string(), v.trim().to_string());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_extracts_name_and_description() {
        let body =
            "---\nname: draft-pr-first\ndescription: Open a draft PR first.\n---\n\n# Body\n";
        let fm = parse_frontmatter(body);
        assert_eq!(fm.get("name").map(String::as_str), Some("draft-pr-first"));
        assert_eq!(
            fm.get("description").map(String::as_str),
            Some("Open a draft PR first.")
        );
    }

    #[test]
    fn frontmatter_keeps_colons_in_value() {
        // Descriptions often contain `:`; split must be on the first one.
        let body = "---\ndescription: Use this: now\n---\n";
        let fm = parse_frontmatter(body);
        assert_eq!(
            fm.get("description").map(String::as_str),
            Some("Use this: now")
        );
    }

    #[test]
    fn frontmatter_absent_returns_empty() {
        let fm = parse_frontmatter("# No frontmatter here\n");
        assert!(fm.is_empty());
    }

    #[test]
    fn skill_urls_match_expected_shape() {
        assert_eq!(
            skill_rendered_url("draft-pr-first"),
            "https://joshrotenberg.github.io/roba/skills/draft-pr-first.html"
        );
        assert_eq!(
            skill_raw_url("draft-pr-first"),
            "https://raw.githubusercontent.com/joshrotenberg/roba/main/skills/draft-pr-first/SKILL.md"
        );
    }

    #[test]
    fn agent_urls_match_expected_shape() {
        assert_eq!(
            agent_rendered_url("roba-runner"),
            "https://joshrotenberg.github.io/roba/agents/roba-runner.html"
        );
        assert_eq!(
            agent_raw_url("roba-runner"),
            "https://raw.githubusercontent.com/joshrotenberg/roba/main/agents/roba-runner/AGENT.md"
        );
    }

    #[test]
    fn kind_based_urls_delegate_to_named_helpers() {
        assert_eq!(rendered_url(SKILLS, "foo"), skill_rendered_url("foo"));
        assert_eq!(raw_url(SKILLS, "foo"), skill_raw_url("foo"));
        assert_eq!(rendered_url(AGENTS, "bar"), agent_rendered_url("bar"));
        assert_eq!(raw_url(AGENTS, "bar"), agent_raw_url("bar"));
    }

    #[test]
    fn installable_excludes_top_level_files() {
        let readme: Entry = ("README", "README.md", "body");
        let skill: Entry = ("draft-pr-first", "draft-pr-first/SKILL.md", "body");
        assert!(!is_installable(&readme));
        assert!(is_installable(&skill));
    }
}
