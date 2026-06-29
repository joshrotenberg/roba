//! Session continuation and permission preset application.
//!
//! These are small mutators on a [`QueryCommand`] -- they translate
//! [`AskArgs`] flags (`-c` / `-c=ID`, `--fork`, `--readonly`,
//! `--full-auto`) into the matching builder method calls.

use claude_wrapper::{Effort, PermissionMode, QueryCommand, RetryPolicy};

use crate::cli::{AskArgs, EffortLevel, PermMode};

/// True when a continue/resume request will be silently defeated by an
/// anonymous (unnamed) worktree: the worktree mints a fresh dir every
/// run, so `-c` finds no prior session there. A NAMED worktree is stable
/// and composes fine, so it is excluded. `--session-id` takes precedence
/// over continue, so it is excluded too.
pub fn continue_defeated_by_anon_worktree(args: &AskArgs) -> bool {
    args.session_id.is_none()
        && args.continue_session.is_some()
        && matches!(args.worktree, Some(None))
}

/// Apply session-related flags (-c / -c=ID, --fork), the model
/// override (--model), the subagent override (--agent), and then
/// permission-related flags. Returns the configured QueryCommand.
pub fn apply_session(mut cmd: QueryCommand, args: &AskArgs) -> QueryCommand {
    // Session selection. clap's conflicts guarantee at most one of
    // {continue/resume, --session-id} is set, so these are mutually
    // exclusive arms: continue/resume picks up an existing session;
    // --session-id assigns a caller-chosen UUID to a new one. The
    // auto-derived `.name(...)` (display label) is applied elsewhere and
    // coexists with either -- name (display) and session-id (UUID) are
    // independent.
    if let Some(id) = &args.session_id {
        cmd = cmd.session_id(id.clone());
    } else {
        match &args.continue_session {
            None => {}                                      // fresh session
            Some(None) => cmd = cmd.continue_session(),     // most recent in cwd
            Some(Some(id)) => cmd = cmd.resume(id.clone()), // specific id
        }
    }
    if args.fork {
        cmd = cmd.fork_session();
    }
    if let Some(m) = &args.model {
        cmd = cmd.model(m.clone());
    }
    if let Some(m) = &args.fallback_model {
        cmd = cmd.fallback_model(m.clone());
    }
    if let Some(e) = args.effort {
        cmd = cmd.effort(match e {
            EffortLevel::Low => Effort::Low,
            EffortLevel::Medium => Effort::Medium,
            EffortLevel::High => Effort::High,
            EffortLevel::Xhigh => Effort::Xhigh,
            EffortLevel::Max => Effort::Max,
        });
    }
    if let Some(name) = &args.agent {
        cmd = cmd.agent(name.clone());
    }
    if let Some(name) = &args.worktree {
        if continue_defeated_by_anon_worktree(args) {
            // Warn-not-block (stderr only; stdout/--json stay byte-clean).
            eprintln!(
                "warning: -c/--resume with an anonymous --worktree starts a fresh worktree each run, so there is no prior session to continue. Use a named worktree (-w NAME) or drop --worktree."
            );
        }
        cmd = match name {
            Some(n) => cmd.worktree_named(n.clone()),
            None => cmd.worktree(),
        };
    }
    if let Some(ref text) = args.system_prompt {
        cmd = cmd.system_prompt(text.clone());
    }
    // Append the user's `--append-system-prompt` and the built-in agent
    // notice as ONE combined value (claude-wrapper's append_system_prompt
    // is a setter, so a second call would clobber the first -- see
    // compose_append_system_prompt).
    if let Some(text) = compose_append_system_prompt(args) {
        cmd = cmd.append_system_prompt(text);
    }
    if args.show_thinking && (args.stream || args.trace.is_some()) {
        cmd = cmd.include_partial_messages();
    }
    if args.no_retry {
        // Force a per-command retry policy of a single attempt. This
        // overrides any client-level default (the wrapper resolves
        // per-command policy first), so a transient failure surfaces
        // immediately instead of being silently re-tried with backoff.
        // `max_attempts(1)` means "no retries."
        cmd = cmd.retry(RetryPolicy::new().max_attempts(1));
    }
    if let Some(n) = args.max_turns {
        cmd = cmd.max_turns(n);
    }
    if let Some(v) = args.max_budget_usd {
        cmd = cmd.max_budget_usd(v);
    }
    if let Some(schema) = &args.json_schema {
        // By this point `args.json_schema` holds the inlined schema JSON
        // (run_ask reads the PATH the flag named and replaces the value
        // with the file contents). claude's `--json-schema` takes inline
        // JSON, so pass it straight through.
        cmd = cmd.json_schema(schema.clone());
    }
    if args.bare {
        cmd = cmd.bare();
    }
    if args.safe_mode {
        cmd = cmd.safe_mode();
    }
    if args.no_session_persistence {
        cmd = cmd.no_session_persistence();
    }
    // Additional tool-access directories: forward each --add-dir path
    // verbatim (claude resolves and reads them). Pure pass-through.
    for d in &args.add_dir {
        cmd = cmd.add_dir(d.clone());
    }
    // MCP servers for this run: forward each --mcp-config path verbatim
    // (claude reads the file), then the strict flag. Pure pass-through.
    for p in &args.mcp_config {
        cmd = cmd.mcp_config(p.clone());
    }
    if args.strict_mcp_config {
        cmd = cmd.strict_mcp_config();
    }
    apply_permissions(cmd, args)
}

/// Apply permission policy.
///
/// The default behavior is "readonly": claude can use Read, Glob,
/// and Grep but nothing else. To open more up, layer additions:
///
/// - `--readonly` -- explicit form of the default; no-op.
/// - `--writable` -- preset that adds Edit + Write.
/// - `--permission-mode MODE` -- pass a specific permission mode to
///   claude (`plan`, `dontAsk`, `auto`, `acceptEdits`, `bypassPermissions`,
///   `default`).
///   Overrides the shortcut flags for the mode itself, but the
///   allowlist (`--allow-tool` / `--writable` preset) still applies.
/// - `--allow-tool` / profile `allow_tool` -- add specific tools or
///   patterns (e.g. `"Bash(git status)"`).
/// - `--deny-tool` / profile `deny_tool` -- block patterns. Not applied
///   under `--full-auto`, which bypasses all checks before the allow/deny
///   lists are built.
/// - `--full-auto` -- bypass everything (overrides above).
pub fn apply_permissions(mut cmd: QueryCommand, args: &AskArgs) -> QueryCommand {
    if args.full_auto {
        return cmd.dangerously_skip_permissions();
    }

    // Apply --permission-mode if set. --full-auto returned above, so the
    // mode only applies on the non-bypass path; it composes with
    // --readonly / --writable (the allow list below still applies).
    if let Some(mode) = args.permission_mode {
        let cw_mode = permission_mode_to_cw(mode);
        cmd = cmd.permission_mode(cw_mode);
    }

    // Always-on safe defaults. --readonly is the explicit form;
    // either way these three are in the allow list.
    let mut allow: Vec<String> = vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()];
    if args.writable {
        push_unique(&mut allow, "Edit");
        push_unique(&mut allow, "Write");
    }
    for t in &args.allow_tool {
        push_unique(&mut allow, t);
    }
    cmd = cmd.allowed_tools(allow);

    if !args.deny_tool.is_empty() {
        cmd = cmd.disallowed_tools(args.deny_tool.clone());
    }

    cmd
}

/// Convert roba's `PermMode` to claude-wrapper's `PermissionMode`.
fn permission_mode_to_cw(mode: PermMode) -> PermissionMode {
    match mode {
        PermMode::AcceptEdits => PermissionMode::AcceptEdits,
        PermMode::Auto => PermissionMode::Auto,
        #[allow(deprecated)]
        PermMode::BypassPermissions => PermissionMode::BypassPermissions,
        PermMode::Default => PermissionMode::Default,
        PermMode::DontAsk => PermissionMode::DontAsk,
        PermMode::Plan => PermissionMode::Plan,
    }
}

fn push_unique(list: &mut Vec<String>, item: &str) {
    if !list.iter().any(|s| s == item) {
        list.push(item.to_string());
    }
}

/// The built-in advisory roba injects into the agent's system prompt by
/// default. It states roba's execution truth: the spawned agent is a
/// single, non-interactive `claude -p` turn, not a persistent harness, so
/// there are no cross-turn background-completion notifications. On by
/// default; suppressed with `--no-agent-notice`, replaced with
/// `--agent-notice` / the `agent_notice` config key. See issue #302.
pub const BUILTIN_AGENT_NOTICE: &str = "You are running as a single, \
non-interactive `claude -p` turn via roba -- not an interactive or persistent \
session. When you stop calling tools and produce your final response, this \
process exits: you will not be re-invoked, and you will not receive \
background-task-completion notifications across turns. If you start \
asynchronous work (for example a detached `roba --detach` worker), either \
block on it synchronously within this turn (`roba show <id> --wait` in the \
foreground), or end your turn by explicitly handing the session handle back \
to the caller. Never background a task and then stop while expecting to be \
auto-resumed.";

/// Resolve the agent-notice text to inject, honoring the disable flag and
/// the content override. Returns `None` when the notice is suppressed
/// (`--no-agent-notice`) or the resolved content is empty
/// (`agent_notice = ""` -- a config-level disable).
fn resolve_agent_notice(args: &AskArgs) -> Option<String> {
    if args.no_agent_notice {
        return None;
    }
    let text = args.agent_notice.as_deref().unwrap_or(BUILTIN_AGENT_NOTICE);
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Compose the final `--append-system-prompt` value: the user's own
/// append text (if any) combined with the built-in agent notice (if
/// enabled).
///
/// claude-wrapper's `append_system_prompt` is a SETTER -- a second call
/// replaces the first -- so the two pieces are joined here (blank line
/// between) and applied once. The notice never clobbers a user's
/// `--append-system-prompt`, and vice versa. Returns `None` when neither
/// piece is present.
pub fn compose_append_system_prompt(args: &AskArgs) -> Option<String> {
    let user = args
        .append_system_prompt
        .as_deref()
        .filter(|s| !s.is_empty());
    let notice = resolve_agent_notice(args);
    match (user, notice) {
        (Some(u), Some(n)) => Some(format!("{u}\n\n{n}")),
        (Some(u), None) => Some(u.to_string()),
        (None, Some(n)) => Some(n),
        (None, None) => None,
    }
}

/// Derive a display name from the resolved prompt for `claude
/// --name`. Shows up in the `claude --resume` picker, the terminal
/// title, and the prompt box. Without it, sessions roba creates
/// are effectively invisible to the picker.
///
/// Shape: `roba: <first 40 chars of the first non-empty line>`. The
/// `roba:` prefix makes sessions distinguishable from interactive
/// Claude Code sessions in the same project's history.
pub fn derive_session_name(prompt: &str) -> String {
    let first_line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let preview: String = if first_line.chars().count() > 40 {
        let head: String = first_line.chars().take(40).collect();
        format!("{head}…")
    } else {
        first_line.to_string()
    };
    if preview.is_empty() {
        "roba".to_string()
    } else {
        format!("roba: {preview}")
    }
}

/// Outcome of resolving a user-supplied `-c <value>` against the known
/// session ids of a project. See [`resolve_session_prefix`].
#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one session id matched (a full id, or a unique prefix);
    /// holds the full id to hand to claude's `--resume`.
    Unique(String),
    /// More than one known session id shares the prefix; holds the
    /// candidates so the caller can list them in an error.
    Ambiguous(Vec<String>),
    /// No known session id has `input` as a prefix. The caller passes the
    /// value through to claude UNCHANGED -- preserving claude's own
    /// session-TITLE resume (roba sets `--name roba...`) and any full id
    /// that belongs to a different project.
    NoMatch,
}

/// Resolve `input` as a git-style session-id prefix against the
/// project's known `available_ids`.
///
/// - unique (case-insensitive) prefix match -> `Unique(full)`;
/// - a full UUID present in the list -> `Unique(itself)` (it is the only
///   id with that 36-char prefix, since all ids share a length);
/// - more than one prefix match -> `Ambiguous(candidates)`;
/// - no prefix match (or empty input) -> `NoMatch`.
///
/// UUID hex is case-insensitive, so matching is too. A non-hex input (a
/// session TITLE) won't prefix any UUID and falls through as `NoMatch`.
///
/// Deliberately lenient, in contrast to `--session-id`'s strict UUID
/// validation (`parse_session_id`, #284): `--session-id` ASSIGNS a new
/// id and must be a well-formed UUID; `-c <prefix>` RESUMES and treats
/// the value as a convenience handle, so a non-matching value is passed
/// on rather than rejected.
pub fn resolve_session_prefix(input: &str, available_ids: &[String]) -> Resolution {
    if input.is_empty() {
        return Resolution::NoMatch;
    }
    let needle = input.to_ascii_lowercase();
    let matches: Vec<String> = available_ids
        .iter()
        .filter(|id| id.to_ascii_lowercase().starts_with(&needle))
        .cloned()
        .collect();
    match matches.len() {
        0 => Resolution::NoMatch,
        1 => Resolution::Unique(matches.into_iter().next().unwrap()),
        _ => Resolution::Ambiguous(matches),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn prefix_unique_match_expands_to_full() {
        let available = ids(&[
            "ef7de917-1234-4abc-8def-000000000001",
            "a1b2c3d4-5678-4abc-8def-000000000002",
        ]);
        assert_eq!(
            resolve_session_prefix("ef7de917", &available),
            Resolution::Unique("ef7de917-1234-4abc-8def-000000000001".to_string())
        );
    }

    #[test]
    fn prefix_ambiguous_returns_all_candidates() {
        let available = ids(&[
            "ef7de917-1234-4abc-8def-000000000001",
            "ef7de917-9999-4abc-8def-000000000002",
            "00000000-0000-4abc-8def-000000000003",
        ]);
        match resolve_session_prefix("ef7de917", &available) {
            Resolution::Ambiguous(candidates) => {
                assert_eq!(candidates.len(), 2);
                assert!(candidates.contains(&"ef7de917-1234-4abc-8def-000000000001".to_string()));
                assert!(candidates.contains(&"ef7de917-9999-4abc-8def-000000000002".to_string()));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn prefix_no_match_falls_through() {
        let available = ids(&["ef7de917-1234-4abc-8def-000000000001"]);
        // A value that prefixes nothing (a different id / a title) is a
        // pass-through, not an error.
        assert_eq!(
            resolve_session_prefix("deadbeef", &available),
            Resolution::NoMatch
        );
    }

    #[test]
    fn prefix_full_uuid_present_matches_itself() {
        let full = "ef7de917-1234-4abc-8def-000000000001";
        let available = ids(&[full, "a1b2c3d4-5678-4abc-8def-000000000002"]);
        assert_eq!(
            resolve_session_prefix(full, &available),
            Resolution::Unique(full.to_string())
        );
    }

    #[test]
    fn prefix_title_like_input_no_match() {
        // claude's session-TITLE resume passes a non-hex string; it must
        // never prefix a UUID and so falls through unchanged.
        let available = ids(&["ef7de917-1234-4abc-8def-000000000001"]);
        assert_eq!(
            resolve_session_prefix("the real prompt", &available),
            Resolution::NoMatch
        );
    }

    #[test]
    fn prefix_eight_char_displayed_form_resolves() {
        // The exact form roba prints in its footer (`session ef7de917`).
        let available = ids(&[
            "ef7de917-1234-4abc-8def-000000000001",
            "ffffffff-5678-4abc-8def-000000000002",
        ]);
        assert_eq!(
            resolve_session_prefix("ef7de917", &available),
            Resolution::Unique("ef7de917-1234-4abc-8def-000000000001".to_string())
        );
    }

    #[test]
    fn prefix_is_case_insensitive() {
        let available = ids(&["ABCDEF12-1234-4abc-8def-000000000001"]);
        assert_eq!(
            resolve_session_prefix("abcdef12", &available),
            Resolution::Unique("ABCDEF12-1234-4abc-8def-000000000001".to_string())
        );
    }

    #[test]
    fn prefix_empty_input_no_match() {
        let available = ids(&["ef7de917-1234-4abc-8def-000000000001"]);
        assert_eq!(resolve_session_prefix("", &available), Resolution::NoMatch);
    }

    #[test]
    fn prefix_empty_id_list_no_match() {
        assert_eq!(resolve_session_prefix("ef7de917", &[]), Resolution::NoMatch);
    }

    #[test]
    fn name_short_prompt_passes_through() {
        assert_eq!(derive_session_name("hello"), "roba: hello");
    }

    #[test]
    fn name_truncates_at_40_chars_with_ellipsis() {
        let prompt = "this is a fairly long prompt that should get cut off somewhere";
        let name = derive_session_name(prompt);
        assert!(name.starts_with("roba: "), "got: {name}");
        assert!(name.ends_with('…'), "got: {name}");
        let body = name.trim_start_matches("roba: ").trim_end_matches('…');
        assert_eq!(body.chars().count(), 40, "got body: {body:?}");
    }

    #[test]
    fn name_uses_first_nonempty_line() {
        let prompt = "\n\n   \nthe real prompt\nignored continuation";
        assert_eq!(derive_session_name(prompt), "roba: the real prompt");
    }

    #[test]
    fn name_empty_prompt_falls_back_to_bare_roba() {
        assert_eq!(derive_session_name(""), "roba");
        assert_eq!(derive_session_name("   \n  \n"), "roba");
    }

    #[test]
    fn show_thinking_gated_on_stream_or_trace() {
        // `--show-thinking` only sets claude's --include-partial-messages
        // when streaming (--stream) or tracing (--trace); on the default
        // non-streaming path claude rejects --include-partial-messages, so
        // the flag must NOT be forwarded. Assert via the derived Debug of
        // the resolved QueryCommand.
        use crate::cli::Cli;
        use clap::Parser;

        let apply = |argv: &[&str]| {
            let cli = Cli::try_parse_from(argv).unwrap();
            format!("{:?}", apply_session(QueryCommand::new("hi"), &cli.ask))
        };

        // Default path: gated off.
        assert!(
            apply(&["roba", "--show-thinking", "prompt"])
                .contains("include_partial_messages: false")
        );
        // --stream: gated on.
        assert!(
            apply(&["roba", "--show-thinking", "--stream", "prompt"])
                .contains("include_partial_messages: true")
        );
        // --trace: gated on.
        assert!(
            apply(&[
                "roba",
                "--show-thinking",
                "--trace",
                "/tmp/x.jsonl",
                "prompt"
            ])
            .contains("include_partial_messages: true")
        );
    }

    // -- agent notice (#302) -----------------------------------------------

    fn ask(argv: &[&str]) -> AskArgs {
        use crate::cli::Cli;
        use clap::Parser;
        Cli::try_parse_from(argv).unwrap().ask
    }

    #[test]
    fn notice_injected_by_default() {
        let composed = compose_append_system_prompt(&ask(&["roba", "prompt"])).unwrap();
        assert!(
            composed.contains("single, non-interactive"),
            "got: {composed}"
        );
    }

    #[test]
    fn notice_absent_under_no_agent_notice() {
        let args = ask(&["roba", "--no-agent-notice", "prompt"]);
        assert!(compose_append_system_prompt(&args).is_none());
    }

    #[test]
    fn notice_override_replaces_builtin() {
        let mut args = ask(&["roba", "prompt"]);
        args.agent_notice = Some("CUSTOM NOTICE".to_string());
        let composed = compose_append_system_prompt(&args).unwrap();
        assert_eq!(composed, "CUSTOM NOTICE");
        assert!(!composed.contains("single, non-interactive"));
    }

    #[test]
    fn notice_composes_with_user_append() {
        let args = ask(&["roba", "--append-system-prompt", "Be terse.", "prompt"]);
        let composed = compose_append_system_prompt(&args).unwrap();
        assert!(composed.contains("Be terse."), "got: {composed}");
        assert!(
            composed.contains("single, non-interactive"),
            "got: {composed}"
        );
    }

    #[test]
    fn notice_empty_override_injects_nothing() {
        let mut args = ask(&["roba", "prompt"]);
        args.agent_notice = Some(String::new());
        assert!(compose_append_system_prompt(&args).is_none());
    }

    #[test]
    fn notice_empty_override_keeps_user_append_only() {
        let mut args = ask(&["roba", "--append-system-prompt", "Be terse.", "prompt"]);
        args.agent_notice = Some(String::new());
        assert_eq!(
            compose_append_system_prompt(&args).as_deref(),
            Some("Be terse.")
        );
    }

    #[test]
    fn apply_session_appends_notice_to_command() {
        // End-to-end: the notice reaches the QueryCommand on the default path.
        let dbg = format!(
            "{:?}",
            apply_session(QueryCommand::new("hi"), &ask(&["roba", "prompt"]))
        );
        assert!(dbg.contains("single, non-interactive"), "got: {dbg}");
    }

    // -- anonymous-worktree continue/resume guard (#328) ------------------

    #[test]
    fn anon_worktree_defeats_continue_most_recent() {
        // `-c` (most recent) + bare `-w` (anonymous) -> true.
        assert!(continue_defeated_by_anon_worktree(&ask(&[
            "roba", "-c", "-w", "-p", "prompt"
        ])));
    }

    #[test]
    fn anon_worktree_defeats_resume_specific() {
        // `-c=ID` (resume specific) + bare `-w` (anonymous) -> true.
        assert!(continue_defeated_by_anon_worktree(&ask(&[
            "roba",
            "-c=abc123",
            "-w",
            "-p",
            "prompt"
        ])));
    }

    #[test]
    fn named_worktree_does_not_defeat_continue() {
        // `-c` + named `-w=NAME` (stable dir) -> false.
        assert!(!continue_defeated_by_anon_worktree(&ask(&[
            "roba",
            "-c",
            "-w=mybranch",
            "-p",
            "prompt"
        ])));
    }

    #[test]
    fn anon_worktree_alone_is_fine() {
        // bare `-w` with no continue/resume -> false.
        assert!(!continue_defeated_by_anon_worktree(&ask(&[
            "roba", "-w", "-p", "prompt"
        ])));
    }

    #[test]
    fn continue_alone_is_fine() {
        // `-c` with no worktree -> false.
        assert!(!continue_defeated_by_anon_worktree(&ask(&[
            "roba", "-c", "-p", "prompt"
        ])));
    }

    #[test]
    fn session_id_takes_precedence_over_continue() {
        // `--session-id` wins over continue (apply_session arms it first),
        // so even with continue set the guard must NOT fire. clap forbids
        // `--session-id` together with `-c`, so set continue_session by hand
        // to model the precedence path.
        let mut args = ask(&[
            "roba",
            "--session-id",
            "ef7de917-1234-4abc-8def-000000000001",
            "-w",
            "-p",
            "prompt",
        ]);
        args.continue_session = Some(None);
        assert!(!continue_defeated_by_anon_worktree(&args));
    }

    #[test]
    fn name_handles_unicode_correctly() {
        // 40 chars of CJK; truncation must use char boundaries, not byte
        let prompt = "あ".repeat(50);
        let name = derive_session_name(&prompt);
        // "roba: " + 40 "あ" + "…"
        let body = name.trim_start_matches("roba: ").trim_end_matches('…');
        assert_eq!(body.chars().count(), 40);
    }
}
