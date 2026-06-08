//! Config data types: the [`Profile`] bundle, its [`WorktreeSetting`]
//! field type, the per-file [`ConfigFile`] split, and the merged
//! [`Pool`] view across every source.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::aliases::Alias;
use crate::cli::EffortLevel;

/// A profile = a bundle of optional defaults. Used both for top-level
/// keys (the unnamed "defaults" baseline) and for named
/// `[profile.NAME]` overlays.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prepend: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub append: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attach: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_diff: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_log: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_status: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_auto: Option<bool>,
    /// `-c` / `--continue`. TOML key is `continue` (a Rust keyword,
    /// so the struct field uses a non-keyword name). Accepts
    /// `continue = true` / `false` (continue the most recent session)
    /// or `continue = "session-id"` (resume a specific id).
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_session: Option<ContinueSetting>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_tool: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deny_tool: Vec<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub vars: HashMap<String, String>,
    /// Override the claude model (alias or full id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Effort level (`--effort`). Controls the cost/quality tradeoff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortLevel>,
    /// Pin a claude-code subagent by name (`--agent`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub echo: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plain: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<bool>,
    /// Default N for `--editor-history` when `-e` is on. 0 disables
    /// the preamble entirely. Only consulted with `-e`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_history: Option<usize>,
    /// Run every session in a fresh git worktree (`-w` / `--worktree`).
    /// TOML accepts either `worktree = true` (claude generates the
    /// name) or `worktree = "NAME"` (pinned name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeSetting>,
    /// Disable wrapper-level auto-retry on transient failures
    /// (`--no-retry`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_retry: Option<bool>,
    /// Minimal-overhead mode: skip hooks, LSP, plugin sync, CLAUDE.md
    /// auto-discovery, auto-memory, and keychain reads (`--bare`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bare: Option<bool>,
    /// Write the spawned session's streaming events to this path as
    /// JSONL (`--trace`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<PathBuf>,
    /// Override the per-model rates table for the footer dollar figure
    /// (`--rates-file`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rates_file: Option<PathBuf>,
    /// Omit the dollar figure from the footer (`--no-dollars`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_dollars: Option<bool>,
    /// Skip the agent frontmatter permission check (`--no-agent-check`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_agent_check: Option<bool>,
    /// Preset for unattended file-mutating dispatch (`--dispatch`):
    /// implies `--full-auto`, `--worktree`, and `--fresh`, with
    /// per-flag overrides respected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<bool>,
    /// Set claude's `--permission-mode` (`--permission-mode MODE`).
    /// Accepts: `default`, `acceptEdits`, `dontAsk`, `plan`, `auto`.
    /// The shortcut flags (`readonly`, `writable`, `full_auto`) take
    /// precedence at the CLI layer; this key applies when none of the
    /// shortcuts are set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionModeConfig>,
    /// Replace the default system prompt entirely (`--system-prompt`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Append to the default system prompt (`--append-system-prompt`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
}

/// Profile value for the `permission_mode` field. Serializes as a
/// string matching the `--permission-mode` CLI values.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PermissionModeConfig {
    /// Auto-accept file edits (`acceptEdits`).
    AcceptEdits,
    /// Model-driven permission decisions (`auto`).
    Auto,
    /// Bypass all permission checks (`bypassPermissions`). Deprecated
    /// upstream; prefer `full_auto = true` for this effect.
    BypassPermissions,
    /// Default interactive permissions (`default`).
    Default,
    /// Accept all allowed tools without prompting (`dontAsk`).
    DontAsk,
    /// Plan mode: show a plan before executing (`plan`).
    Plan,
}

/// Profile value for the `worktree` field. Mirrors the CLI's
/// `--worktree[=NAME]` shape: `true` means presence-only, a string
/// means a pinned name, `false` is an explicit off.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum WorktreeSetting {
    Enabled(bool),
    Named(String),
}

/// Profile value for the `continue` field. Mirrors the CLI's
/// `-c[=ID]` shape: `true` continues the most recent session, a
/// string resumes that specific session id, `false` is an explicit
/// off (stay fresh).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ContinueSetting {
    MostRecent(bool),
    Specific(String),
}

impl Profile {
    /// True if every field is at its default (no overrides).
    pub fn is_empty(&self) -> bool {
        self.prepend.is_empty()
            && self.append.is_empty()
            && self.attach.is_empty()
            && self.git_diff.is_none()
            && self.git_log.is_none()
            && self.git_status.is_none()
            && self.readonly.is_none()
            && self.writable.is_none()
            && self.full_auto.is_none()
            && self.continue_session.is_none()
            && self.allow_tool.is_empty()
            && self.deny_tool.is_empty()
            && self.vars.is_empty()
            && self.model.is_none()
            && self.effort.is_none()
            && self.agent.is_none()
            && self.stream.is_none()
            && self.show_thinking.is_none()
            && self.echo.is_none()
            && self.plain.is_none()
            && self.quiet.is_none()
            && self.json.is_none()
            && self.editor_history.is_none()
            && self.worktree.is_none()
            && self.no_retry.is_none()
            && self.bare.is_none()
            && self.trace.is_none()
            && self.rates_file.is_none()
            && self.no_dollars.is_none()
            && self.no_agent_check.is_none()
            && self.dispatch.is_none()
            && self.permission_mode.is_none()
            && self.system_prompt.is_none()
            && self.append_system_prompt.is_none()
    }

    /// Merge `other` on top of `self`. Used to layer roba.toml files
    /// closer-to-cwd on top of farther-from-cwd ones.
    ///
    /// - `Option<T>`: when `other.field.is_some()`, it overrides
    /// - `Vec<T>`: concat (self's items first, then other's)
    /// - `HashMap` (vars): per-key merge; other wins on key conflict
    pub fn merge_in(&mut self, other: Profile) {
        let Profile {
            mut prepend,
            mut append,
            mut attach,
            git_diff,
            git_log,
            git_status,
            readonly,
            writable,
            full_auto,
            continue_session,
            mut allow_tool,
            mut deny_tool,
            vars,
            model,
            effort,
            agent,
            stream,
            show_thinking,
            echo,
            plain,
            quiet,
            json,
            editor_history,
            worktree,
            no_retry,
            bare,
            trace,
            rates_file,
            no_dollars,
            no_agent_check,
            dispatch,
            permission_mode,
            system_prompt,
            append_system_prompt,
        } = other;

        self.prepend.append(&mut prepend);
        self.append.append(&mut append);
        self.attach.append(&mut attach);
        if git_diff.is_some() {
            self.git_diff = git_diff;
        }
        if git_log.is_some() {
            self.git_log = git_log;
        }
        if git_status.is_some() {
            self.git_status = git_status;
        }
        if readonly.is_some() {
            self.readonly = readonly;
        }
        if writable.is_some() {
            self.writable = writable;
        }
        if full_auto.is_some() {
            self.full_auto = full_auto;
        }
        if continue_session.is_some() {
            self.continue_session = continue_session;
        }
        self.allow_tool.append(&mut allow_tool);
        self.deny_tool.append(&mut deny_tool);
        for (k, v) in vars {
            self.vars.insert(k, v);
        }
        if model.is_some() {
            self.model = model;
        }
        if effort.is_some() {
            self.effort = effort;
        }
        if agent.is_some() {
            self.agent = agent;
        }
        if stream.is_some() {
            self.stream = stream;
        }
        if show_thinking.is_some() {
            self.show_thinking = show_thinking;
        }
        if echo.is_some() {
            self.echo = echo;
        }
        if plain.is_some() {
            self.plain = plain;
        }
        if quiet.is_some() {
            self.quiet = quiet;
        }
        if json.is_some() {
            self.json = json;
        }
        if editor_history.is_some() {
            self.editor_history = editor_history;
        }
        if worktree.is_some() {
            self.worktree = worktree;
        }
        if no_retry.is_some() {
            self.no_retry = no_retry;
        }
        if bare.is_some() {
            self.bare = bare;
        }
        if trace.is_some() {
            self.trace = trace;
        }
        if rates_file.is_some() {
            self.rates_file = rates_file;
        }
        if no_dollars.is_some() {
            self.no_dollars = no_dollars;
        }
        if no_agent_check.is_some() {
            self.no_agent_check = no_agent_check;
        }
        if dispatch.is_some() {
            self.dispatch = dispatch;
        }
        if permission_mode.is_some() {
            self.permission_mode = permission_mode;
        }
        if system_prompt.is_some() {
            self.system_prompt = system_prompt;
        }
        if append_system_prompt.is_some() {
            self.append_system_prompt = append_system_prompt;
        }
    }
}

/// One loaded `roba.toml` file: top-level keys parsed as the
/// unnamed defaults profile, plus the map of `[profile.NAME]`
/// overlays.
#[derive(Debug, Default, Clone)]
pub struct ConfigFile {
    pub defaults: Profile,
    pub profile: HashMap<String, Profile>,
    pub alias: HashMap<String, Alias>,
    /// `[session]` table: a flat `NAME = "<uuid>"` map binding a stable
    /// handle to a claude session id. roba only ever READS this map
    /// (`--session NAME` / `ROBA_SESSION`); it never writes it.
    pub session: HashMap<String, String>,
}

/// Resolved view across every config source for one roba invocation.
#[derive(Debug, Default, Clone)]
pub struct Pool {
    /// Merged top-level defaults across all loaded files.
    pub defaults: Profile,
    /// Merged named profiles. When the same name appears in multiple
    /// files, fields are merged per [`Profile::merge_in`].
    pub profiles: HashMap<String, Profile>,
    /// Merged aliases. Unlike profiles, an alias definition does not
    /// merge field-by-field: when the same name appears in multiple
    /// files, the closest-to-cwd file's definition wins wholesale.
    pub aliases: HashMap<String, Alias>,
    /// Merged `[session]` name -> uuid bindings. Like aliases, the
    /// closest-to-cwd file's binding wins wholesale on a name collision.
    /// Declarative + read-only: `--session NAME` resolves NAME here.
    pub sessions: HashMap<String, String>,
    /// Source files that contributed, in load order. Used by
    /// `roba profile path` for diagnostics.
    pub sources: Vec<PathBuf>,
}

impl Pool {
    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    /// Test helper: parse a TOML string the way `load_file` would
    /// (splitting top-level vs `[profile.*]`).
    fn load_file_from_str(s: &str) -> Result<ConfigFile> {
        let mut value: toml::Value = toml::from_str(s)?;
        let profile_map: HashMap<String, Profile> = if let toml::Value::Table(t) = &mut value {
            match t.remove("profile") {
                Some(v) => v.try_into()?,
                None => HashMap::new(),
            }
        } else {
            HashMap::new()
        };
        let alias_map: HashMap<String, Alias> = if let toml::Value::Table(t) = &mut value {
            match t.remove("alias") {
                Some(v) => v.try_into()?,
                None => HashMap::new(),
            }
        } else {
            HashMap::new()
        };
        let session_map: HashMap<String, String> = if let toml::Value::Table(t) = &mut value {
            match t.remove("session") {
                Some(v) => v.try_into()?,
                None => HashMap::new(),
            }
        } else {
            HashMap::new()
        };
        let defaults: Profile = value.try_into()?;
        Ok(ConfigFile {
            defaults,
            profile: profile_map,
            alias: alias_map,
            session: session_map,
        })
    }

    // -- Profile parsing ---------------------------------------------------

    #[test]
    fn parse_minimal_profile() {
        let toml = r#"
[profile.review]
readonly = true
git_diff = true
"#;
        let cfg = load_file_from_str(toml).unwrap();
        let p = &cfg.profile["review"];
        assert_eq!(p.readonly, Some(true));
        assert_eq!(p.git_diff, Some(true));
        assert!(p.attach.is_empty());
    }

    #[test]
    fn parse_profile_with_vars_and_lists() {
        let toml = r#"
[profile.fancy]
prepend = ["/tmp/a", "/tmp/b"]
attach = ["**/*.rs"]
git_log = 5

[profile.fancy.vars]
NAME = "Josh"
TICKET = "ABC-123"
"#;
        let cfg = load_file_from_str(toml).unwrap();
        let p = &cfg.profile["fancy"];
        assert_eq!(p.prepend.len(), 2);
        assert_eq!(p.attach, vec!["**/*.rs"]);
        assert_eq!(p.git_log, Some(5));
        assert_eq!(p.vars.get("NAME"), Some(&"Josh".to_string()));
    }

    #[test]
    fn parse_rejects_unknown_fields_in_profile() {
        let toml = r#"
[profile.bad]
typo_field = "oops"
"#;
        assert!(load_file_from_str(toml).is_err());
    }

    #[test]
    fn parse_rejects_unknown_top_level_keys() {
        let toml = r#"
prependz = ["/tmp/a"]
"#;
        assert!(load_file_from_str(toml).is_err());
    }

    #[test]
    fn parse_continue_field_uses_renamed_key() {
        let toml = r#"
[profile.persist]
continue = true
"#;
        let cfg = load_file_from_str(toml).unwrap();
        assert_eq!(
            cfg.profile["persist"].continue_session,
            Some(ContinueSetting::MostRecent(true))
        );
    }

    #[test]
    fn parse_continue_field_accepts_specific_id() {
        let toml = r#"
[profile.persist]
continue = "abc12345"
"#;
        let cfg = load_file_from_str(toml).unwrap();
        assert_eq!(
            cfg.profile["persist"].continue_session,
            Some(ContinueSetting::Specific("abc12345".to_string()))
        );
    }

    #[test]
    fn sample_config_parses_and_documents_the_schema() {
        // Drift guard: the embedded roba-config.sample.toml is the canonical
        // config reference (its comments document every key). It must parse
        // through the same path as a real roba.toml -- with deny_unknown_fields
        // on Profile, any renamed/removed key fails here until the sample is
        // updated. Config docs that literally cannot go stale.
        let cfg = load_file_from_str(crate::profile::cmd::STARTER_CONFIG_TOML)
            .expect("roba-config.sample.toml must parse as a valid config");
        for name in ["review", "explain", "commit-msg", "fix-build"] {
            assert!(
                cfg.profile.contains_key(name),
                "sample is missing [profile.{name}]"
            );
        }
        for name in ["review", "commit-msg", "r"] {
            assert!(
                cfg.alias.contains_key(name),
                "sample is missing [alias.{name}]"
            );
        }
        // A profile-scoped var and an alias arg schema round-trip.
        assert_eq!(
            cfg.profile["commit-msg"]
                .vars
                .get("STYLE")
                .map(String::as_str),
            Some("imperative, concise, no marketing")
        );
        assert_eq!(cfg.alias["review"].args, vec!["pr".to_string()]);
    }

    #[test]
    fn parse_allow_tool_singular() {
        let toml = r#"
[profile.x]
allow_tool = ["Edit", "Write"]
deny_tool = ["WebFetch"]
"#;
        let cfg = load_file_from_str(toml).unwrap();
        let p = &cfg.profile["x"];
        assert_eq!(p.allow_tool, vec!["Edit".to_string(), "Write".to_string()]);
        assert_eq!(p.deny_tool, vec!["WebFetch".to_string()]);
    }

    #[test]
    fn parse_top_level_defaults() {
        let toml = r#"
readonly = true
attach = ["**/*.rs"]

[profile.review]
git_diff = true
"#;
        let cfg = load_file_from_str(toml).unwrap();
        assert_eq!(cfg.defaults.readonly, Some(true));
        assert_eq!(cfg.defaults.attach, vec!["**/*.rs"]);
        assert_eq!(cfg.profile["review"].git_diff, Some(true));
    }

    // -- Profile merging ---------------------------------------------------

    #[test]
    fn merge_in_concats_lists() {
        let mut a = Profile {
            prepend: vec![PathBuf::from("/a")],
            allow_tool: vec!["Edit".into()],
            ..Default::default()
        };
        let b = Profile {
            prepend: vec![PathBuf::from("/b")],
            allow_tool: vec!["Write".into()],
            ..Default::default()
        };
        a.merge_in(b);
        assert_eq!(a.prepend, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(a.allow_tool, vec!["Edit".to_string(), "Write".to_string()]);
    }

    #[test]
    fn merge_in_other_wins_on_scalars() {
        let mut a = Profile {
            readonly: Some(false),
            git_log: Some(3),
            ..Default::default()
        };
        let b = Profile {
            readonly: Some(true),
            git_log: None,
            git_diff: Some(true),
            ..Default::default()
        };
        a.merge_in(b);
        assert_eq!(a.readonly, Some(true)); // other overrode
        assert_eq!(a.git_log, Some(3)); // other was None, keep self
        assert_eq!(a.git_diff, Some(true)); // self was None, take other
    }

    #[test]
    fn merge_in_vars_other_wins_per_key() {
        let mut vars_a = HashMap::new();
        vars_a.insert("X".to_string(), "from_a".to_string());
        vars_a.insert("Y".to_string(), "from_a".to_string());
        let mut a = Profile {
            vars: vars_a,
            ..Default::default()
        };
        let mut vars_b = HashMap::new();
        vars_b.insert("X".to_string(), "from_b".to_string());
        vars_b.insert("Z".to_string(), "from_b".to_string());
        let b = Profile {
            vars: vars_b,
            ..Default::default()
        };
        a.merge_in(b);
        assert_eq!(a.vars["X"], "from_b"); // other won
        assert_eq!(a.vars["Y"], "from_a"); // kept from self
        assert_eq!(a.vars["Z"], "from_b"); // added by other
    }
}
