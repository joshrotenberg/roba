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
//! Alias *args* interpolated into a `$(...)` region are treated as
//! **data, not shell code**: every `${...}` value is POSIX
//! single-quoted before the command reaches `sh -c`, so a
//! `;`/`&&`/backtick-bearing arg cannot break out and execute
//! (closes #287). Put shell syntax in the template literal, not in an
//! arg.
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

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::cli::{AliasAction, AliasDraftArgs, AskArgs};
use crate::profile::{self, Pool};

/// Built-in subcommand names, **derived from the clap command tree** so
/// the set can never drift from the real dispatch table. A user alias
/// matching one of these is shadowed (the built-in wins);
/// [`profile::load_pool`] warns when it loads such an alias.
///
/// The set is every real [`crate::cli::SubCommand`] variant clap exposes
/// (via `get_subcommands()`), each subcommand's visible aliases (if any),
/// plus clap's implicit `help` subcommand. The `external_subcommand`
/// catch-all (the very mechanism that dispatches user aliases) has no
/// fixed name and is intentionally absent. Computed once and cached.
pub fn builtin_subcommands() -> &'static [String] {
    use clap::CommandFactory;
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let cmd = crate::cli::Cli::command();
        let mut names: Vec<String> = Vec::new();
        for sub in cmd.get_subcommands() {
            names.push(sub.get_name().to_string());
            names.extend(sub.get_visible_aliases().map(str::to_string));
        }
        // clap synthesizes a `help` subcommand; it is not a `SubCommand`
        // variant, so add it explicitly.
        names.push("help".to_string());
        names.sort();
        names.dedup();
        names
    })
}

/// True if `name` collides with a built-in subcommand (see
/// [`builtin_subcommands`]).
pub fn is_builtin_subcommand(name: &str) -> bool {
    builtin_subcommands().iter().any(|n| n == name)
}

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
        // `draft` makes an async claude call and is routed through
        // [`run_draft`] by [`crate::dispatch`], never here.
        AliasAction::Draft(_) => unreachable!("alias draft is dispatched via run_draft"),
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
pub(crate) fn render_alias_toml(name: &str, alias: &Alias) -> Result<String> {
    use std::collections::HashMap;
    let mut wrapper: HashMap<String, HashMap<String, Alias>> = HashMap::new();
    let mut inner: HashMap<String, Alias> = HashMap::new();
    inner.insert(name.to_string(), alias.clone());
    wrapper.insert("alias".to_string(), inner);
    toml::to_string_pretty(&wrapper).context("re-serializing alias")
}

// ---------------------------------------------------------------------------
// Draft: claude-assisted, parse-validated alias generation
// ---------------------------------------------------------------------------

/// Run `roba alias draft DESCRIPTION`.
///
/// Deterministic bookends around one generation: build a prompt from the
/// bundled (parse-tested) alias schema + the user's words, make a single
/// lean claude call, then validate the output with roba's REAL `Alias`
/// deserializer (`deny_unknown_fields`) and normalize it back through
/// `render_alias_toml`. stdout gets only the canonical block; collision
/// warnings and write confirmations go to stderr. No retry loop.
pub async fn run_draft(args: AliasDraftArgs) -> Result<()> {
    // 1. Deterministic prompt: schema-by-example + the user's description.
    let prompt = draft_prompt(&args.description);

    // 2. One lean claude call -- NOT routed through `run_ask`. The shared
    //    draft core handles the read-only, no-session, stdin-fed posture.
    let raw = crate::draft::generate(prompt, args.model.as_deref(), "roba: alias draft").await?;

    // 3. Validate with the real deserializer; require exactly one entry.
    let (name, alias) = parse_drafted_alias(&raw)?;

    // 4. Normalize so stdout is canonical regardless of model formatting.
    let block = render_alias_toml(&name, &alias)?;

    let is_builtin = is_builtin_subcommand(&name);
    match &args.write {
        Some(target) => {
            let path = match target {
                Some(p) => p.clone(),
                None => profile::user_config_path().ok_or_else(|| {
                    anyhow::anyhow!(
                        "--write: cannot locate your user config; pass an explicit path (`--write PATH`)"
                    )
                })?,
            };
            // A built-in name is unreachable as a verb -- hard error.
            if is_builtin {
                bail!(
                    "alias `{name}` collides with a built-in subcommand and would be unreachable; pick another name"
                );
            }
            // A duplicate `[alias.NAME]` table in the target would hard-break
            // the next config load -- hard error.
            if file_defines_alias(&path, &name)? {
                bail!(
                    "{} already defines [alias.{name}]; refusing to append a duplicate (it would break the next config load)",
                    path.display()
                );
            }
            crate::draft::append_block(&path, &block)?;
            eprintln!("wrote [alias.{name}] to {}", path.display());
        }
        None => {
            // Print mode: collisions are warnings, not errors.
            if is_builtin {
                eprintln!(
                    "warning: alias `{name}` collides with a built-in subcommand; the built-in wins, so the alias would be unreachable"
                );
            } else if profile::load_pool()?.aliases.contains_key(&name) {
                eprintln!(
                    "warning: alias `{name}` already exists in your config pool; this draft would shadow or duplicate it"
                );
            }
        }
    }

    // stdout = the block only, byte-clean (pipeable to `>> roba.toml`).
    print!("{block}");
    Ok(())
}

/// Build the deterministic generation prompt: the bundled alias schema
/// (by example, so it cannot drift from the real deserializer) + the
/// user's description + firm single-block output instructions.
fn draft_prompt(description: &str) -> String {
    let schema = alias_sample_section();
    format!(
        "You are generating a single roba alias definition in TOML.\n\n\
         roba aliases are user-defined verbs. Here is the alias schema, \
         documented by example -- this is the ONLY allowed shape, do not \
         invent fields:\n\n\
         {schema}\n\n\
         The user wants an alias for: {description}\n\n\
         Output requirements (follow exactly):\n\
         - Produce EXACTLY ONE `[alias.NAME]` TOML block and nothing else.\n\
         - Pick a short, memorable kebab-case or single-word NAME from the description.\n\
         - Use ONLY the fields shown above (description, agent, template, flags, args).\n\
         - The block must be valid TOML that parses against that schema.\n\
         - Do NOT wrap the output in markdown code fences.\n\
         - Do NOT include any prose, comments, or explanation -- only the TOML block."
    )
}

/// Slice the Aliases section out of the bundled, parse-tested sample
/// config (the `# Aliases` banner through the start of `# Named
/// sessions`). Falls back to the whole sample if those markers move, so
/// a future sample reshuffle degrades to "embed everything" rather than
/// silently shipping an empty schema.
fn alias_sample_section() -> String {
    const SAMPLE: &str = crate::profile::STARTER_CONFIG_TOML;
    let start = SAMPLE.find("# Aliases");
    let end = SAMPLE.find("# Named sessions");
    match (start, end) {
        (Some(s), Some(e)) if s < e => SAMPLE[s..e].trim_end().to_string(),
        _ => SAMPLE.trim_end().to_string(),
    }
}

/// Parse a drafted alias block. Strip optional markdown fences (a model
/// may add them despite instructions), deserialize via the real
/// `{ alias: { NAME = Alias } }` shape so `deny_unknown_fields` on
/// [`Alias`] polices hallucinated keys, and require EXACTLY one entry.
/// On any failure, the error carries the raw model output for stderr.
fn parse_drafted_alias(raw: &str) -> Result<(String, Alias)> {
    use std::collections::HashMap;
    #[derive(Deserialize)]
    struct Wrapper {
        #[serde(default)]
        alias: HashMap<String, Alias>,
    }
    let cleaned = crate::draft::strip_code_fences(raw);
    let wrapper: Wrapper = toml::from_str(&cleaned).map_err(|e| {
        anyhow::anyhow!("drafted alias did not parse: {e}\n\n--- raw model output ---\n{raw}")
    })?;
    let mut entries: Vec<(String, Alias)> = wrapper.alias.into_iter().collect();
    match entries.len() {
        1 => Ok(entries.pop().expect("len checked == 1")),
        0 => {
            bail!("drafted output defined no [alias.NAME] block\n\n--- raw model output ---\n{raw}")
        }
        n => bail!(
            "drafted output defined {n} alias blocks (expected exactly one)\n\n--- raw model output ---\n{raw}"
        ),
    }
}

/// True when `path` already defines `[alias.name]`. A missing file
/// defines nothing. Unknown top-level tables (profiles, sessions) are
/// ignored -- only the alias map is probed -- but malformed TOML surfaces
/// as an error rather than a silent "no".
fn file_defines_alias(path: &Path, name: &str) -> Result<bool> {
    use std::collections::HashMap;
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        alias: HashMap<String, Alias>,
    }
    if !path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading --write target {}", path.display()))?;
    let probe: Probe = toml::from_str(&text)
        .with_context(|| format!("--write target {} is not valid TOML", path.display()))?;
    Ok(probe.alias.contains_key(name))
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

/// Guard a lone bare word against being a *typo* of a built-in
/// subcommand.
///
/// roba accepts bare-word prompts (`roba hello`), so a single token that
/// is neither a subcommand nor a known alias normally becomes the
/// prompt. But a close edit-distance miss of a real subcommand
/// (`worktrees` for `worktree`, `histroy` for `history`) is almost
/// always a typo, not an intended prompt; left alone it fires a
/// surprising, billable claude call (#353). When the token is a single
/// edit (under `damerau_osa`) from a built-in subcommand, return a
/// "did you mean" message so [`crate::dispatch`] can bail instead of
/// prompting.
///
/// Gated identically to [`bare_alias_candidate`] (single whitespace-free
/// word, no `-f` / `-e`), and only reached after it -- so an exact alias
/// has already won. Matches built-in names ONLY, never alias names:
/// built-ins are a small fixed set unlikely to collide with an intended
/// prompt, whereas user aliases can be short, prompt-like words. The
/// threshold is one OSA edit -- tighter than `unknown_alias_message`'s
/// Levenshtein 3 -- because the bare-word path has a high prior that the
/// input is a real prompt; OSA keeps transposition typos (`histroy`)
/// caught at distance 1 while leaving distance-2 lookalikes (`hello` vs
/// `help`) as prompts. The `-p` escape hatch in the message keeps a
/// genuine bare-word prompt recoverable (it sets the prompt flag, not
/// the positional, so this guard does not fire on it).
pub fn bare_subcommand_typo(ask: &AskArgs) -> Option<String> {
    let prompt = ask.prompt.as_deref()?;
    if prompt.is_empty() || prompt.chars().any(char::is_whitespace) {
        return None;
    }
    if ask.file.is_some() || ask.editor {
        return None;
    }
    // An exact built-in name is routed by clap and never reaches the
    // bare-word path; guard defensively for direct callers/tests.
    if is_builtin_subcommand(prompt) {
        return None;
    }
    let suggestion = closest_matches(prompt, builtin_subcommands().to_vec(), damerau_osa, 1, 1)
        .into_iter()
        .next()?;
    Some(format!(
        "`{prompt}` is not a roba command; did you mean `{suggestion}`?\n       \
         (to send it as a prompt: roba -p \"{prompt}\")"
    ))
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

/// The context a `${...}` value is being expanded into.
///
/// In [`Ctx::Prompt`] a resolved value is inserted verbatim -- it is
/// inert prompt text. In [`Ctx::Shell`] the value is interpolated into a
/// `$(...)` command string that will be handed to `sh -c`, so every
/// substituted value is POSIX single-quoted to land as ONE inert shell
/// token: alias args are *data, not shell code* (closes #287). The
/// author's literal template text, the `$$`->`$` escape, and nested
/// `$()` composition are unaffected -- only resolved arg values are
/// quoted, and only in `Shell` context.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Prompt,
    Shell,
}

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
///
/// # Security
///
/// Any arg value substituted *inside* a `$(...)` region is POSIX
/// single-quoted before reaching the shell, so it is data and never
/// shell code: `$(gh pr diff ${pr})` with `pr = "244; rm -rf ~"`
/// expands to `gh pr diff '244; rm -rf ~'` -- one inert argument, no
/// exec. Shell syntax must live in the template literal, not in an arg.
pub fn expand_template(template: &str, schema: &[String], args: &[String]) -> Result<String> {
    expand_in(template, schema, args, Ctx::Prompt)
}

/// Inner expander, parameterized by [`Ctx`]. The public
/// [`expand_template`] enters at [`Ctx::Prompt`]; a `$(...)` region
/// always recurses at [`Ctx::Shell`] (its body feeds `sh -c`).
fn expand_in(template: &str, schema: &[String], args: &[String], ctx: Ctx) -> Result<String> {
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
                        match ctx {
                            Ctx::Prompt => out.push_str(&resolve_var(&name, schema, args)),
                            Ctx::Shell => out.push_str(&resolve_var_shell(&name, schema, args)),
                        }
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
                        // Shell context single-quotes every resolved arg
                        // so it is data, not code (closes #287).
                        let cmd = expand_in(&cmd, schema, args, Ctx::Shell)?;
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

/// Shell-context variable resolution: like [`resolve_var`] but every
/// value is POSIX single-quoted (via [`shell_quote`]) so it lands as one
/// inert `sh` token. `${@}` quotes each arg *separately* and space-joins
/// them (`'a' 'b' 'c'`), preserving word separation; `${N}` / `${name}`
/// quote their single resolved value (empty / unknown -> `''`).
fn resolve_var_shell(name: &str, schema: &[String], args: &[String]) -> String {
    if name == "@" {
        return args
            .iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
    }
    shell_quote(&resolve_var(name, schema, args))
}

/// POSIX single-quote `s` so it is exactly one inert `sh` token: wrap in
/// single quotes and rewrite every embedded `'` as `'\''` (close-quote,
/// escaped literal quote, reopen-quote). An empty string becomes `''`.
/// Bulletproof for `sh`: inside single quotes nothing is special, so no
/// metacharacter (`;`, `&&`, backtick, `$(...)`, glob, newline) can act.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
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

/// Names from `candidates` within `max_dist` of `name` under the `dist`
/// metric, closest first (ties broken alphabetically), capped at `take`.
fn closest_matches(
    name: &str,
    candidates: impl IntoIterator<Item = String>,
    dist: impl Fn(&str, &str) -> usize,
    max_dist: usize,
    take: usize,
) -> Vec<String> {
    let mut scored: Vec<(usize, String)> = candidates
        .into_iter()
        .map(|c| (dist(name, &c), c))
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    scored
        .into_iter()
        .filter(|(d, _)| *d <= max_dist)
        .take(take)
        .map(|(_, c)| c)
        .collect()
}

/// Build the "no built-in or alias named X" error, with up to three
/// close matches (built-ins + aliases) by Levenshtein distance.
fn unknown_alias_message(name: &str, pool: &Pool) -> String {
    let mut candidates: Vec<String> = builtin_subcommands().to_vec();
    candidates.extend(pool.aliases.keys().cloned());
    let suggestions = closest_matches(name, candidates, levenshtein, 3, 3);
    if suggestions.is_empty() {
        format!("no built-in or alias named `{name}`")
    } else {
        format!(
            "no built-in or alias named `{name}`; did you mean: {}?",
            suggestions.join(", ")
        )
    }
}

/// Optimal String Alignment distance: Levenshtein plus adjacent
/// transposition as a single edit. A transposed pair (`histroy` vs
/// `history`) costs 1 here but 2 under plain [`levenshtein`], which
/// matters for the bare-word typo guard: it lets a threshold of 1 catch
/// the common single-typo classes (insert / delete / substitute /
/// transpose) without the distance-2 false positives plain Levenshtein
/// would admit (`hello` is distance 2 from `help`).
fn damerau_osa(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let mut v = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[n][m]
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

    // --- #287: shell-context quoting (args are data, not shell code) ---

    #[test]
    fn shell_quote_wraps_plain_value() {
        assert_eq!(shell_quote("244"), "'244'");
        assert_eq!(shell_quote("hello world"), "'hello world'");
    }

    #[test]
    fn shell_quote_empty_is_two_quotes() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quote() {
        // close-quote, escaped literal quote, reopen-quote.
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_quote_neutralizes_metacharacters() {
        // Every shell metacharacter ends up inside single quotes, where
        // nothing is special -- one inert token, no exec.
        for raw in [
            "X; rm -rf ~",
            "a && b",
            "`whoami`",
            "$(whoami)",
            "a | b",
            "x > /etc/passwd",
            "* .rs",
        ] {
            let q = shell_quote(raw);
            assert!(q.starts_with('\'') && q.ends_with('\''), "{q}");
            // No raw single-quote survives unescaped inside the body, and
            // the value is bracketed by exactly the wrapping quotes.
            assert_eq!(q, format!("'{}'", raw.replace('\'', "'\\''")), "{q}");
        }
    }

    #[test]
    fn shell_ctx_quotes_injection_payload_as_one_token() {
        // The verified repro from #287: the arg value must land as a
        // SINGLE quoted token inside the command, never as live shell
        // code. We expand the COMMAND BODY directly in Shell context, so
        // no shell runs -- we prove the transform, not the execution.
        let payload = "X; touch /tmp/PWN; echo END";
        let cmd = expand_in(
            "echo got-${n}",
            &s(&["n"]),
            &[payload.to_string()],
            Ctx::Shell,
        )
        .unwrap();
        assert_eq!(cmd, "echo got-'X; touch /tmp/PWN; echo END'");
        // The dangerous `;` only appears inside the quoted token; there
        // is no unquoted `;` that sh would treat as a command separator.
        let outside_quotes: String = {
            let mut keep = String::new();
            let mut in_q = false;
            for c in cmd.chars() {
                if c == '\'' {
                    in_q = !in_q;
                } else if !in_q {
                    keep.push(c);
                }
            }
            keep
        };
        assert!(
            !outside_quotes.contains(';'),
            "unquoted `;`: {outside_quotes}"
        );
    }

    #[test]
    fn shell_ctx_quotes_various_metachar_args() {
        for payload in ["a; b", "a && b", "`id`", "$(id)", "it's", "has space"] {
            let cmd =
                expand_in("run ${x}", &s(&["x"]), &[payload.to_string()], Ctx::Shell).unwrap();
            assert_eq!(cmd, format!("run {}", shell_quote(payload)), "{cmd}");
        }
    }

    #[test]
    fn shell_ctx_quotes_each_at_arg_separately() {
        // ${@} preserves word separation: each arg is its own quoted
        // token, so an injected separator in one arg can't merge them.
        let cmd = expand_in("cmd ${@}", &[], &s(&["a b", "c;d", "e"]), Ctx::Shell).unwrap();
        assert_eq!(cmd, "cmd 'a b' 'c;d' 'e'");
    }

    #[test]
    fn shell_ctx_legit_path_still_substitutes_quoted() {
        // The #248 guarantee survives: the value IS substituted into the
        // command (just quoted now). `$(gh pr diff ${pr})` with pr=244
        // yields the command `gh pr diff '244'`.
        let cmd = expand_in("gh pr diff ${pr}", &s(&["pr"]), &s(&["244"]), Ctx::Shell).unwrap();
        assert_eq!(cmd, "gh pr diff '244'");
    }

    #[test]
    fn shell_ctx_empty_var_quotes_to_empty_token() {
        // An unset/out-of-range var becomes an explicit empty token, not
        // a bare gap that could let the next word collide.
        let cmd = expand_in("echo ${nope}", &[], &[], Ctx::Shell).unwrap();
        assert_eq!(cmd, "echo ''");
    }

    #[test]
    fn shell_ctx_dollar_escape_unchanged() {
        // `$$` is still the shell-`$` escape inside a command body; it is
        // not an arg value, so quoting does not touch it.
        let cmd = expand_in("echo $$HOME", &[], &[], Ctx::Shell).unwrap();
        assert_eq!(cmd, "echo $HOME");
    }

    #[test]
    fn expand_template_injection_payload_does_not_execute() {
        // End-to-end with a BENIGN payload: the injected `echo INJECTED`
        // must appear as inert text in the result, NOT run as a second
        // command. If quoting failed, sh would run `echo INJECTED` and
        // the marker would be on its own line / absent from the token.
        let out = expand_template(
            "$(echo got-${n})",
            &s(&["n"]),
            &["A; echo INJECTED".to_string()],
        )
        .unwrap();
        // `echo got-'A; echo INJECTED'` prints the whole thing literally.
        assert_eq!(out, "got-A; echo INJECTED");
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
    fn damerau_osa_counts_transposition_as_one() {
        // The motivating distinction: a transposition is one OSA edit but
        // two Levenshtein edits.
        assert_eq!(damerau_osa("histroy", "history"), 1);
        assert_eq!(levenshtein("histroy", "history"), 2);
        // A deletion is one under both.
        assert_eq!(damerau_osa("worktrees", "worktree"), 1);
        // The false positive OSA-1 must NOT admit: hello vs help is 2.
        assert_eq!(damerau_osa("hello", "help"), 2);
    }

    fn ask_from(args: &[&str]) -> AskArgs {
        use clap::Parser;
        crate::cli::Cli::try_parse_from(args)
            .expect("parse cli")
            .ask
    }

    #[test]
    fn bare_subcommand_typo_flags_deletion_typo() {
        let msg = bare_subcommand_typo(&ask_from(&["roba", "worktrees"]))
            .expect("worktrees is one deletion from worktree");
        assert!(msg.contains("`worktrees` is not a roba command"), "{msg}");
        assert!(msg.contains("did you mean `worktree`"), "{msg}");
        assert!(msg.contains("roba -p \"worktrees\""), "{msg}");
    }

    #[test]
    fn bare_subcommand_typo_flags_transposition_typo() {
        let msg = bare_subcommand_typo(&ask_from(&["roba", "histroy"]))
            .expect("histroy is one transposition from history");
        assert!(msg.contains("did you mean `history`"), "{msg}");
    }

    #[test]
    fn bare_subcommand_typo_ignores_far_word() {
        // A genuine bare-word prompt. `hello` is OSA-2 from `help`, so it
        // must stay a prompt, not get hijacked into a suggestion.
        assert!(bare_subcommand_typo(&ask_from(&["roba", "hello"])).is_none());
    }

    #[test]
    fn bare_subcommand_typo_ignores_multiword_prompt() {
        assert!(bare_subcommand_typo(&ask_from(&["roba", "explain this code"])).is_none());
    }

    #[test]
    fn bare_subcommand_typo_ignores_exact_builtin() {
        // Defensive: clap routes an exact name, but a direct caller must
        // not get a self-suggestion. Build the AskArgs via a far word,
        // then override the prompt (an exact name like `worktree` fails to
        // parse on its own -- it needs a sub-action).
        let mut ask = ask_from(&["roba", "hello"]);
        ask.prompt = Some("worktree".to_string());
        assert!(bare_subcommand_typo(&ask).is_none());
    }

    #[test]
    fn bare_subcommand_typo_ignores_when_file_or_editor() {
        let mut ask = ask_from(&["roba", "worktrees"]);
        ask.file = Some("notes.txt".into());
        assert!(bare_subcommand_typo(&ask).is_none());

        let mut ask = ask_from(&["roba", "worktrees"]);
        ask.editor = true;
        assert!(bare_subcommand_typo(&ask).is_none());
    }

    #[test]
    fn parse_drafted_alias_accepts_one_block() {
        let (name, alias) =
            parse_drafted_alias("[alias.echo]\ndescription = \"echo it\"\ntemplate = \"say ${@}\"")
                .unwrap();
        assert_eq!(name, "echo");
        assert_eq!(alias.description.as_deref(), Some("echo it"));
        assert_eq!(alias.template.as_deref(), Some("say ${@}"));
    }

    #[test]
    fn parse_drafted_alias_strips_fences_first() {
        let (name, _) =
            parse_drafted_alias("```toml\n[alias.echo]\ndescription = \"e\"\n```").unwrap();
        assert_eq!(name, "echo");
    }

    #[test]
    fn parse_drafted_alias_rejects_zero_entries() {
        let err = parse_drafted_alias("# nothing here").unwrap_err();
        assert!(
            format!("{err:#}").contains("no [alias.NAME] block"),
            "{err:#}"
        );
    }

    #[test]
    fn parse_drafted_alias_rejects_two_entries() {
        let raw = "[alias.a]\ndescription = \"a\"\n[alias.b]\ndescription = \"b\"";
        let err = parse_drafted_alias(raw).unwrap_err();
        assert!(format!("{err:#}").contains("2 alias blocks"), "{err:#}");
    }

    #[test]
    fn parse_drafted_alias_rejects_unknown_field() {
        // `deny_unknown_fields` on `Alias` rejects a hallucinated key; the
        // raw output is echoed for the user.
        let raw = "[alias.x]\ndescription = \"x\"\nmade_up_key = true";
        let err = parse_drafted_alias(raw).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("did not parse"), "{msg}");
        assert!(msg.contains("made_up_key"), "{msg}");
        assert!(msg.contains("raw model output"), "{msg}");
    }

    #[test]
    fn alias_sample_section_includes_schema_examples() {
        let section = alias_sample_section();
        assert!(section.contains("[alias.review]"), "got:\n{section}");
        assert!(section.contains("${pr}"), "got:\n{section}");
        // Sliced before the named-sessions block.
        assert!(!section.contains("Named sessions"), "got:\n{section}");
    }

    #[test]
    fn file_defines_alias_detects_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roba.toml");
        std::fs::write(
            &path,
            "[profile.x]\nreadonly = true\n\n[alias.review]\ndescription = \"r\"\n",
        )
        .unwrap();
        assert!(file_defines_alias(&path, "review").unwrap());
        assert!(!file_defines_alias(&path, "nope").unwrap());
    }

    #[test]
    fn file_defines_alias_missing_file_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");
        assert!(!file_defines_alias(&path, "anything").unwrap());
    }

    #[test]
    fn builtin_collision_is_detectable() {
        assert!(is_builtin_subcommand("history"));
        assert!(!is_builtin_subcommand("my-custom-verb"));
    }

    #[test]
    fn builtin_set_is_derived_from_the_clap_tree() {
        // Future-proofing: every current SubCommand variant name (plus
        // clap's synthesized `help`) must appear in the derived set, so
        // the shadow-warning can never silently miss a real subcommand.
        for name in [
            "history",
            "last",
            "cost",
            "profile",
            "alias",
            "doctor",
            "completions",
            "worktree",
            "show",
            "config",
            "help",
        ] {
            assert!(
                is_builtin_subcommand(name),
                "derived builtin set is missing `{name}`"
            );
        }
        // Regression for #268: `skill`/`agent` were removed as
        // subcommands in #130 and must NOT shadow user aliases anymore;
        // the `external_subcommand` catch-all has no fixed name.
        assert!(!is_builtin_subcommand("skill"));
        assert!(!is_builtin_subcommand("agent"));
        assert!(!is_builtin_subcommand("external"));
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
