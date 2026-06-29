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

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use toml::Value;

use crate::aliases::{self, Alias};
use crate::cli::{ConfigExplainArgs, ConfigInitArgs, ConfigShowArgs};
use crate::profile::{self, ConfigFile, Pool, Profile};

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

    // 1. Load the config ALREADY inherited by this cwd (user config + any
    //    upstream `roba.toml`). The project file being drafted does not
    //    exist yet (a `--write` clobber is refused above), so the pool is
    //    exactly "what is inherited". Render it as canonical roba.toml; an
    //    empty pool renders blank, in which case we inject nothing.
    let pool = profile::load_pool()?;
    let rendered = render_merged_pool(&pool)?;
    let inherited = (!rendered.trim().is_empty()).then_some(rendered);

    // 2. Deterministic prompt: the whole bundled schema + optional steer +
    //    the inherited pool (so the draft is a delta, not a re-statement).
    let prompt = init_prompt(args.description.as_deref(), inherited.as_deref());

    // 3. One lean claude call with a read-only window onto the project,
    //    bounded turns. NOT routed through `run_ask`.
    let raw = crate::draft::generate_inspecting(prompt, args.model.as_deref(), "roba: config init")
        .await?;

    // 4. Validate the WHOLE file through the real per-file deserializer.
    //    Comments are preserved (we print the cleaned text, not a
    //    re-serialization), so a starter file stays human-readable.
    let content = validate_config(&raw)?;

    // 5. Optional write -- never clobbering an existing config.
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

/// Run `roba config show [--json]`.
///
/// The read-only merged-pool view: load the whole config pool for the cwd
/// (user config + every `roba.toml` up to the git root, already merged by
/// [`crate::profile::load_pool`]) and print it as one canonical roba.toml.
/// A short header naming the auto-applied profile and the source files goes
/// to STDERR so stdout stays byte-clean and re-parseable (principle #2).
/// `--json` swaps the TOML body for the uniform `{ version: 1, result }`
/// envelope on stdout.
///
/// With `--sources`, the EFFECTIVE/provenance view is produced by
/// `run_show_sources` instead (part 2 of #330). Without it, this is the
/// part-1 structural/merged view.
pub fn run_show(args: ConfigShowArgs) -> Result<()> {
    if args.sources.is_some() {
        return run_show_sources(args);
    }

    let pool = profile::load_pool()?;
    let active = profile::cmd::active_profile(&pool)?;

    if args.json {
        return emit_json(&pool, active.map(|(name, _)| name));
    }

    print_show_header(&active, &pool);
    print!("{}", render_merged_pool(&pool)?);
    Ok(())
}

/// Print the shared `config show` header to STDERR (metadata, so stdout
/// stays a clean, pipeable body). Names the auto-applied profile and the
/// source files that contributed.
fn print_show_header(active: &Option<(String, &'static str)>, pool: &Pool) {
    match active {
        Some((name, reason)) => eprintln!("active profile: {name} ({reason})"),
        None => eprintln!("no profile auto-applies"),
    }
    if pool.sources.is_empty() {
        eprintln!("sources: (none)");
    } else {
        eprintln!("sources:");
        for s in &pool.sources {
            eprintln!("  {}", s.display());
        }
    }
}

/// Run `roba config show --sources [KEY]`: the EFFECTIVE top-level config
/// with per-key provenance (part 2 of #330).
///
/// Collapses every config layer -- each file's top-level keys (farther-
/// from-cwd first), then the auto-applied `[profile.NAME]`, then the
/// `ROBA_*` env overrides -- into the final value a bare `roba "..."` in
/// this cwd would start from, annotating each key with the layer that won
/// it. Bare `--sources` prints all set keys; `--sources KEY` prints one.
fn run_show_sources(args: ConfigShowArgs) -> Result<()> {
    let key_filter = args.sources.clone().flatten();

    let pool = profile::load_pool()?;
    let active = profile::cmd::active_profile(&pool)?;
    let layers = profile::pool::load_layers()?;
    let env: HashMap<String, String> = std::env::vars().collect();

    let (effective, unattributed) = compute_effective(&layers, &active, &pool, &env)?;

    if args.json {
        return emit_sources_json(&effective, key_filter.as_deref());
    }

    print_show_header(&active, &pool);
    if !unattributed.is_empty() {
        eprintln!(
            "note: env vars set but not attributed per-key: {}",
            unattributed.join(", ")
        );
    }

    match key_filter {
        Some(key) => match effective.get(&key) {
            Some((value, source)) => print!("{}", render_key_line(&key, value, source)?),
            // Genuinely unset by any layer -> claude's own default applies.
            None => eprintln!("{key} is not set by any config layer"),
        },
        None => {
            for (key, (value, source)) in &effective {
                print!("{}", render_key_line(key, value, source)?);
            }
        }
    }
    Ok(())
}

/// One precedence-layer entry: the toml key, its value, and a human label
/// for the layer that set it. Kept flat (rather than per-layer tables) so
/// the env layer -- whose every key carries its own `ROBA_X` label -- fits
/// the same shape as the file/profile layers.
type LayerEntry = (String, Value, String);

/// The effective top-level config: each key some layer set, mapped to its
/// final `(value, winning source label)`, sorted for stable output.
type Effective = BTreeMap<String, (Value, String)>;

/// Collapse the ordered config layers into the effective top-level config
/// with provenance: a sorted map `key -> (value, winning source label)`
/// over the union of keys ANY layer set, plus the list of `ROBA_*` vars
/// that were set but could not be cleanly attributed to a single key.
///
/// Layers are walked lowest-precedence first (each file's top-level keys
/// farther-from-cwd first, then the auto-applied profile, then env), so a
/// later layer that sets a key overwrites the record -- the highest setter
/// wins, exactly as the real resolver collapses them.
fn compute_effective(
    layers: &[(PathBuf, ConfigFile)],
    active: &Option<(String, &'static str)>,
    pool: &Pool,
    env: &HashMap<String, String>,
) -> Result<(Effective, Vec<String>)> {
    let mut entries: Vec<LayerEntry> = Vec::new();

    // 1. Each file's top-level defaults, farther-from-cwd first.
    for (path, cfg) in layers {
        let label = format!("{} (top-level)", path.display());
        for (k, v) in profile_table(&cfg.defaults)? {
            entries.push((k, v, label.clone()));
        }
    }

    // 2. The auto-applied profile (merged across files), overlaid on top.
    if let Some((name, _)) = active
        && let Some(profile) = pool.get(name)
    {
        let label = format!("[profile.{name}]");
        for (k, v) in profile_table(profile)? {
            entries.push((k, v, label.clone()));
        }
    }

    // 3. The ROBA_* env overrides (highest config layer).
    let mut unattributed = Vec::new();
    entries.extend(env_entries(env, &mut unattributed));

    let mut effective: BTreeMap<String, (Value, String)> = BTreeMap::new();
    for (k, v, label) in entries {
        effective.insert(k, (v, label));
    }
    Ok((effective, unattributed))
}

/// Serialize a [`Profile`] to its TOML table. Only SET fields appear
/// (every field is `skip_serializing_if`), so a key's PRESENCE here means
/// this layer set it -- the generic per-key walk that avoids hand-matching
/// every field.
fn profile_table(profile: &Profile) -> Result<toml::Table> {
    match Value::try_from(profile).context("serializing a config layer")? {
        Value::Table(t) => Ok(t),
        _ => Ok(toml::Table::new()),
    }
}

/// Render one effective key as `key = value  # source`, reusing TOML's own
/// scalar/array formatting (so strings stay quoted, bools/numbers bare, and
/// the line is still valid TOML with a trailing comment).
fn render_key_line(key: &str, value: &Value, source: &str) -> Result<String> {
    let mut t = toml::Table::new();
    t.insert(key.to_string(), value.clone());
    let body = toml::to_string(&t).context("serializing an effective key")?;
    Ok(format!("{}  # {source}\n", body.trim_end_matches('\n')))
}

/// Emit the effective view as the uniform `{ version: 1, result }` JSON
/// envelope, where `result` is `{ effective: { KEY: { value, source } } }`.
/// Filtered to one KEY when `key_filter` is set (an unset KEY yields an
/// empty `effective` map, never an error).
fn emit_sources_json(effective: &Effective, key_filter: Option<&str>) -> Result<()> {
    #[derive(Serialize)]
    struct KeyProvenance<'a> {
        value: &'a Value,
        source: &'a str,
    }
    #[derive(Serialize)]
    struct Report<'a> {
        effective: BTreeMap<&'a str, KeyProvenance<'a>>,
    }

    let mut map: BTreeMap<&str, KeyProvenance> = BTreeMap::new();
    for (k, (v, s)) in effective {
        if key_filter.is_some_and(|f| f != k) {
            continue;
        }
        map.insert(
            k.as_str(),
            KeyProvenance {
                value: v,
                source: s,
            },
        );
    }
    let report = Report { effective: map };
    println!(
        "{}",
        serde_json::to_string_pretty(&crate::VersionedResult::new(&report))?
    );
    Ok(())
}

/// How a `ROBA_*` env var's string value maps to a TOML value for the
/// effective view. Mirrors the value semantics in [`crate::env`] so the
/// provenance view agrees with what the env layer actually applies.
#[derive(Clone, Copy)]
enum EnvKind {
    /// Truthy-only bool: a truthy value sets `true`, anything else is no-op.
    Bool,
    /// Any non-empty string.
    Str,
    /// Comma-separated list of strings.
    List,
    /// Non-negative integer (invalid values ignored).
    Uint,
    /// Floating-point number (invalid values ignored).
    Float,
    /// `-c`-style: truthy -> `true`, falsy -> unset, else a specific id.
    Continue,
    /// `-w`-style: truthy -> `true`, falsy -> unset, else a pinned name.
    Worktree,
    /// One of a fixed set (case-insensitive); an unrecognized value is
    /// left unattributed rather than guessed.
    Choice(&'static [&'static str]),
}

/// The `ROBA_*` env var -> Profile-key mapping, by the [`crate::env`]
/// naming convention. Covers every single-field knob; the per-key `vars`
/// map (`ROBA_VAR_*`) and the selection vars (`ROBA_PROFILE`/`SESSION`/
/// `SESSION_ID`) are intentionally absent -- they are not top-level config
/// knobs, and `vars` is surfaced via the unattributed note instead.
const ENV_MAP: &[(&str, &str, EnvKind)] = &[
    ("ROBA_PREPEND", "prepend", EnvKind::List),
    ("ROBA_APPEND", "append", EnvKind::List),
    ("ROBA_ATTACH", "attach", EnvKind::List),
    ("ROBA_GIT_DIFF", "git_diff", EnvKind::Bool),
    ("ROBA_GIT_LOG", "git_log", EnvKind::Uint),
    ("ROBA_GIT_STATUS", "git_status", EnvKind::Bool),
    ("ROBA_READONLY", "readonly", EnvKind::Bool),
    ("ROBA_WRITABLE", "writable", EnvKind::Bool),
    ("ROBA_FULL_AUTO", "full_auto", EnvKind::Bool),
    ("ROBA_CONTINUE", "continue", EnvKind::Continue),
    ("ROBA_ALLOW_TOOL", "allow_tool", EnvKind::List),
    ("ROBA_DENY_TOOL", "deny_tool", EnvKind::List),
    ("ROBA_MODEL", "model", EnvKind::Str),
    (
        "ROBA_EFFORT",
        "effort",
        EnvKind::Choice(&["low", "medium", "high", "xhigh", "max"]),
    ),
    ("ROBA_AGENT", "agent", EnvKind::Str),
    ("ROBA_STREAM", "stream", EnvKind::Bool),
    ("ROBA_SHOW_THINKING", "show_thinking", EnvKind::Bool),
    ("ROBA_ECHO", "echo", EnvKind::Bool),
    ("ROBA_PLAIN", "plain", EnvKind::Bool),
    ("ROBA_QUIET", "quiet", EnvKind::Bool),
    ("ROBA_JSON", "json", EnvKind::Bool),
    ("ROBA_EDITOR_HISTORY", "editor_history", EnvKind::Uint),
    ("ROBA_WORKTREE", "worktree", EnvKind::Worktree),
    ("ROBA_NO_RETRY", "no_retry", EnvKind::Bool),
    ("ROBA_BARE", "bare", EnvKind::Bool),
    ("ROBA_SAFE_MODE", "safe_mode", EnvKind::Bool),
    ("ROBA_TRACE", "trace", EnvKind::Str),
    ("ROBA_RATES_FILE", "rates_file", EnvKind::Str),
    ("ROBA_NO_DOLLARS", "no_dollars", EnvKind::Bool),
    ("ROBA_NO_AGENT_CHECK", "no_agent_check", EnvKind::Bool),
    (
        "ROBA_PERMISSION_MODE",
        "permission_mode",
        EnvKind::Choice(&[
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "default",
            "dontAsk",
            "plan",
        ]),
    ),
    ("ROBA_SYSTEM_PROMPT", "system_prompt", EnvKind::Str),
    (
        "ROBA_APPEND_SYSTEM_PROMPT",
        "append_system_prompt",
        EnvKind::Str,
    ),
    ("ROBA_MAX_TURNS", "max_turns", EnvKind::Uint),
    ("ROBA_MAX_BUDGET_USD", "max_budget_usd", EnvKind::Float),
    ("ROBA_TIMEOUT", "timeout", EnvKind::Uint),
    ("ROBA_JSON_SCHEMA", "json_schema", EnvKind::Str),
    ("ROBA_MCP_CONFIG", "mcp_config", EnvKind::List),
    ("ROBA_STRICT_MCP_CONFIG", "strict_mcp_config", EnvKind::Bool),
    ("ROBA_ADD_DIR", "add_dir", EnvKind::List),
    ("ROBA_FALLBACK_MODEL", "fallback_model", EnvKind::Str),
    (
        "ROBA_NO_SESSION_PERSISTENCE",
        "no_session_persistence",
        EnvKind::Bool,
    ),
    ("ROBA_NO_AGENT_NOTICE", "no_agent_notice", EnvKind::Bool),
    ("ROBA_AGENT_NOTICE", "agent_notice", EnvKind::Str),
];

fn env_truthy(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

fn env_falsy(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// Build the env-layer entries from the environment, mirroring
/// [`crate::env`]'s value semantics. Pushes any set-but-unmappable
/// `ROBA_*` var name onto `unattributed` (an unrecognized `Choice` value,
/// or the per-key `ROBA_VAR_*` map) rather than guessing.
fn env_entries(env: &HashMap<String, String>, unattributed: &mut Vec<String>) -> Vec<LayerEntry> {
    let mut out = Vec::new();
    for &(var, key, kind) in ENV_MAP {
        let raw = match env.get(var) {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };
        let value = match kind {
            EnvKind::Bool => env_truthy(raw).then_some(Value::Boolean(true)),
            EnvKind::Str => Some(Value::String(raw.clone())),
            EnvKind::List => {
                let items: Vec<Value> = raw
                    .split(',')
                    .map(|p| p.trim())
                    .filter(|p| !p.is_empty())
                    .map(|p| Value::String(p.to_string()))
                    .collect();
                (!items.is_empty()).then_some(Value::Array(items))
            }
            EnvKind::Uint => raw.parse::<u64>().ok().map(|n| Value::Integer(n as i64)),
            EnvKind::Float => raw.parse::<f64>().ok().map(Value::Float),
            EnvKind::Continue | EnvKind::Worktree => {
                if env_truthy(raw) {
                    Some(Value::Boolean(true))
                } else if env_falsy(raw) {
                    None
                } else {
                    Some(Value::String(raw.clone()))
                }
            }
            EnvKind::Choice(allowed) => {
                match allowed.iter().find(|a| a.eq_ignore_ascii_case(raw)) {
                    Some(canon) => Some(Value::String((*canon).to_string())),
                    None => {
                        unattributed.push(var.to_string());
                        None
                    }
                }
            }
        };
        if let Some(v) = value {
            out.push((key.to_string(), v, format!("env ({var})")));
        }
    }
    if env.keys().any(|k| k.starts_with("ROBA_VAR_")) {
        unattributed.push("ROBA_VAR_* (per-key vars)".to_string());
    }
    out
}

/// Render the merged pool as one canonical, re-parseable roba.toml.
///
/// Serialized SECTION BY SECTION and concatenated rather than as one
/// flattened struct: in a single TOML document every scalar/array key must
/// precede any `[table]`, so the top-level defaults block (which may end in
/// a `[vars]` sub-table) is emitted first, then the `[profile.NAME]`,
/// `[alias.NAME]`, and `[session]` tables. Each section is sorted by name
/// for stable output.
fn render_merged_pool(pool: &Pool) -> Result<String> {
    let mut out = String::new();

    // 1. Top-level defaults (scalar/array keys, plus a [vars] table when
    //    set). Must precede every table below.
    if !pool.defaults.is_empty() {
        let block = toml::to_string_pretty(&pool.defaults).context("re-serializing defaults")?;
        push_block(&mut out, &block);
    }

    // 2. [profile.NAME] blocks, sorted by name.
    let mut profiles: Vec<&String> = pool.profiles.keys().collect();
    profiles.sort();
    for name in profiles {
        let block = profile::cmd::render_named_profile(name, &pool.profiles[name])?;
        push_block(&mut out, &block);
    }

    // 3. [alias.NAME] blocks, sorted by name.
    let mut aliases: Vec<&String> = pool.aliases.keys().collect();
    aliases.sort();
    for name in aliases {
        let block = aliases::render_alias_toml(name, &pool.aliases[name])?;
        push_block(&mut out, &block);
    }

    // 4. A single [session] table when any handle is bound.
    if !pool.sessions.is_empty() {
        push_block(&mut out, &render_session_table(&pool.sessions)?);
    }

    Ok(out)
}

/// Append one serialized TOML block, separated from any prior block by a
/// blank line. A blank block is skipped so an empty pool yields empty
/// stdout.
fn push_block(out: &mut String, block: &str) {
    let block = block.trim_end_matches('\n');
    if block.is_empty() {
        return;
    }
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(block);
    out.push('\n');
}

/// Render the merged `[session]` NAME -> uuid map as a sorted TOML table.
fn render_session_table(sessions: &std::collections::HashMap<String, String>) -> Result<String> {
    let sorted: BTreeMap<&String, &String> = sessions.iter().collect();
    let mut wrapper: BTreeMap<&str, BTreeMap<&String, &String>> = BTreeMap::new();
    wrapper.insert("session", sorted);
    toml::to_string_pretty(&wrapper).context("re-serializing sessions")
}

/// Emit the merged pool as the uniform `{ version: 1, result }` JSON
/// envelope on stdout. Maps are sorted (BTreeMap) for stable output.
fn emit_json(pool: &Pool, active_profile: Option<String>) -> Result<()> {
    #[derive(Serialize)]
    struct Report<'a> {
        active_profile: Option<String>,
        sources: Vec<String>,
        defaults: &'a Profile,
        profiles: BTreeMap<&'a String, &'a Profile>,
        aliases: BTreeMap<&'a String, &'a Alias>,
        sessions: BTreeMap<&'a String, &'a String>,
    }

    let report = Report {
        active_profile,
        sources: pool
            .sources
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        defaults: &pool.defaults,
        profiles: pool.profiles.iter().collect(),
        aliases: pool.aliases.iter().collect(),
        sessions: pool.sessions.iter().collect(),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&crate::VersionedResult::new(&report))?
    );
    Ok(())
}

/// Build the generation prompt: the ENTIRE bundled, parse-tested sample
/// config (for a whole file the whole schema is the right grounding) +
/// drafting instructions + the user's steer when given + the config the
/// cwd ALREADY inherits (so the draft is a project-specific delta).
///
/// `inherited` is the rendered, merged inherited pool (user config + any
/// upstream `roba.toml`) when non-empty, else `None`; when `None` the
/// inherited-config section is omitted cleanly (no dangling heading).
fn init_prompt(description: Option<&str>, inherited: Option<&str>) -> String {
    let sample = crate::profile::STARTER_CONFIG_TOML;
    let steering = match description {
        Some(d) => format!("\n\nThe user added this steer; weave it into the draft:\n{d}"),
        None => String::new(),
    };
    let inherited_section = match inherited {
        Some(text) => format!(
            "\n\nThis config is ALREADY inherited by this project (the user \
             config plus any `roba.toml` in a parent directory); roba merges \
             it into every run here automatically:\n\n\
             {text}\n\n\
             Emit only the project-specific DELTA: do NOT re-state keys that \
             are already inherited, and do NOT override an inherited key \
             unless there is a clear project-specific reason to.",
        ),
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
         The three config layers have distinct purposes -- place things in the \
         right one:\n\
         - top-level keys are AMBIENT, always-on knobs for the KIND of work in \
         this scope; they merge per-key across the pool and auto-apply silently \
         to every run, so put only SAFE things here.\n\
         - `[profile.NAME]` is a named, opt-in-able MODE selected as a unit \
         (`--profile NAME`); it is the home for loaded guns, named and deliberate.\n\
         - `[alias.NAME]` is a profile PLUS a prompt template, invoked as a verb; \
         only worth it when there is a template, args, or agent.\n\n\
         First, briefly inspect THIS project with the Read/Glob/Grep tools (the \
         README, the package manifest, the top-level layout) so the draft fits \
         what you see. Skim, do not spelunk.\
         {inherited_section}\n\n\
         Then produce a starter project roba.toml that:\n\
         - Opens with conservative, SAFE top-level defaults appropriate to this project.\n\
         - NEVER puts `full_auto` or an anonymous `worktree` (`worktree = true`) at \
         the TOP LEVEL -- those are loaded guns that would auto-apply silently to \
         every run (and a top-level anonymous worktree silently defeats `-c`/`--resume`). \
         If the steer asks for full-auto or worktree isolation, put them in a NAMED \
         `[profile.NAME]` (e.g. `[profile.worker]`) so they are an explicit, named \
         opt-in. Ship a SAFE default; NAME the loaded guns.\n\
         - Defines a couple of useful `[profile.NAME]` overlays fitted to the project.\n\
         - Defines AT MOST one or two `[alias.NAME]` shortcut verbs, and only if clearly useful.\n\
         - Generates REUSABLE verbs, not a one-shot plan: an `[alias.NAME]` is for \
         something run repeatedly (e.g. `roba review ${{pr}}`). Do NOT emit a numbered \
         family of one-time build steps (`phase0`, `phase1`, ...) -- that is a PLAN, \
         and a plan's home is GitHub issues driven by an orchestrator, not a config \
         file. If the project has a phased roadmap, note it in a `#` comment rather \
         than encoding the plan as aliases.\n\
         - Keeps every profile/permission posture READ-ONLY unless the steer below asks otherwise.\n\
         - Has a brief `#` comment on every key explaining what it does.\n\
         - Does NOT include a `[session]` table (session UUIDs are machine-local).\n\
         - Keeps `template`, `args`, and `flags` to `[alias.NAME]` tables ONLY -- \
         they are alias keys and a `[profile.NAME]` table will REJECT them (a profile \
         that wants a prompt template, args, or extra flags is actually an alias).\n\
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

// ---------------------------------------------------------------------------
// `config explain`: the human-readable narrated view (#357)
// ---------------------------------------------------------------------------

/// Run `roba config explain [--plain]`.
///
/// The human counterpart to `config show`. `show` prints raw, re-parseable
/// TOML (the machine / redirect view, byte-clean stdout); `explain` renders
/// a grouped, annotated narrative for a person eyeballing "what is my
/// effective config and what does it actually do." It is a human view only:
/// the whole layout goes to stdout, but it is never something a script
/// parses -- `show` and `--json` own that.
///
/// Color is on when stdout is a TTY and `NO_COLOR` is unset and `--plain`
/// was not passed (the same gating `render.rs` applies). `--plain` only
/// turns color off; it does NOT fall back to the raw dump.
pub fn run_explain(args: ConfigExplainArgs) -> Result<()> {
    let pool = profile::load_pool()?;
    let active = profile::cmd::active_profile(&pool)?;
    // The authoritative per-file list (the same source `config show
    // --sources` uses) -- numbers the Sources section and attributes each
    // rendered value to the file that set it. The merged `pool.sources`
    // loses that per-file attribution.
    let layers = profile::pool::load_layers()?;
    let painter = Painter {
        color: explain_color(args.plain),
    };
    print!(
        "{}",
        render_explain(&pool, &active, &layers, &knob_descriptions(), &painter)?
    );
    Ok(())
}

/// Whether `config explain` should colorize: off under `--plain`, `NO_COLOR`,
/// or a non-TTY stdout. Mirrors the `color` predicate in [`crate::render`].
fn explain_color(plain: bool) -> bool {
    use std::io::IsTerminal;
    !plain && std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// Tiny ANSI palette gate. When `color` is false every method returns the
/// text untouched, so the same `render_explain` produces both the colored
/// TTY view and the byte-plain `--plain` / piped view (and is testable
/// without a real terminal).
struct Painter {
    color: bool,
}

impl Painter {
    /// Section header: green-bold, matching clap's own `--help` headers.
    fn header(&self, s: &str) -> String {
        self.wrap(s, "\x1b[1;32m")
    }
    /// A config knob / alias name: cyan (clap's literal style).
    fn key(&self, s: &str) -> String {
        self.wrap(s, "\x1b[36m")
    }
    /// Secondary detail (descriptions, sources): dim.
    fn dim(&self, s: &str) -> String {
        self.wrap(s, "\x1b[2m")
    }
    /// A loaded-gun warning marker: bold yellow.
    fn warn(&self, s: &str) -> String {
        self.wrap(s, "\x1b[1;33m")
    }
    fn wrap(&self, s: &str, code: &str) -> String {
        if self.color {
            format!("{code}{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}

/// Map each top-level config knob (by its TOML key) to the terse one-line
/// description clap derives from the `cli.rs` doc comment, so `explain` and
/// `--help` share one source and cannot drift. The key space is the arg id
/// (the `AskArgs` field name), which equals the TOML key for every knob
/// except `continue` (field `continue_session`); [`desc_for`] bridges that
/// one rename.
fn knob_descriptions() -> HashMap<String, String> {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let mut map = HashMap::new();
    for arg in cmd.get_arguments() {
        if let Some(help) = arg.get_help() {
            map.insert(arg.get_id().as_str().to_string(), help.to_string());
        }
    }
    map
}

/// Look up a knob's description by its TOML key, bridging the one serde
/// rename (`continue` -> the `continue_session` arg id).
fn desc_for<'a>(descs: &'a HashMap<String, String>, toml_key: &str) -> Option<&'a str> {
    let arg_id = match toml_key {
        "continue" => "continue_session",
        other => other,
    };
    descs.get(arg_id).map(String::as_str)
}

/// Render a TOML scalar/array value inline (without a trailing key), so a
/// knob reads as `model = "opus"` / `allow_tool = ["Edit", "Write"]`. A
/// sub-table (only `vars` at the top level) renders as `{ ... }`.
fn format_toml_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Integer(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Datetime(d) => d.to_string(),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(format_toml_value).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Table(_) => "{ ... }".to_string(),
    }
}

/// The set keys a profile defines that are "loaded guns" -- task-scoped
/// powers (`full_auto`, any `worktree`) that should be a deliberate opt-in,
/// never silently always-on. Mirrors the `config lint` advisory.
fn loaded_guns(table: &toml::Table) -> Vec<&'static str> {
    let mut guns = Vec::new();
    if table.get("full_auto") == Some(&Value::Boolean(true)) {
        guns.push("full_auto");
    }
    // worktree is a loaded gun only when ENABLED (`true` or a named worktree);
    // `worktree = false` explicitly disables it, so it is not flagged (mirrors
    // how full_auto checks for `true`, not mere key presence).
    if matches!(
        table.get("worktree"),
        Some(Value::Boolean(true)) | Some(Value::String(_))
    ) {
        guns.push("worktree");
    }
    guns
}

/// A compact one-line posture summary of a profile: each set knob as
/// `key` (a true bool) or `key=value`, sorted, comma-joined. Empty profiles
/// summarize as `(no overrides)`.
///
/// Loaded-gun pieces ([`loaded_guns`]) are painted yellow and the rest dim,
/// so a profile's risky knobs catch the eye inline on a color TTY. With color
/// off it is byte-plain (the `[!] loaded gun:` text marker carries the signal),
/// so the risk indicator never depends on color alone.
fn posture_summary(table: &toml::Table, p: &Painter) -> String {
    let guns = loaded_guns(table);
    let mut pieces: Vec<String> = Vec::new();
    for (k, v) in table {
        let piece = match v {
            Value::Boolean(true) => k.clone(),
            _ => format!("{k}={}", format_toml_value(v)),
        };
        if guns.contains(&k.as_str()) {
            pieces.push(p.warn(&piece));
        } else {
            pieces.push(p.dim(&piece));
        }
    }
    if pieces.is_empty() {
        p.dim("(no overrides)")
    } else {
        pieces.join(&p.dim(", "))
    }
}

/// The invocation form shown for an alias verb: `roba NAME <arg> ...` when it
/// has a positional schema, `roba NAME ...` for a template alias with no
/// schema, and `roba NAME <prompt...>` for a flag-shortcut alias (no
/// template -- the args become the prompt).
fn alias_invocation(name: &str, alias: &Alias) -> String {
    if !alias.args.is_empty() {
        let slots: Vec<String> = alias.args.iter().map(|a| format!("<{a}>")).collect();
        format!("roba {name} {}", slots.join(" "))
    } else if alias.template.is_some() {
        format!("roba {name} ...")
    } else {
        format!("roba {name} <prompt...>")
    }
}

/// Per-item source attribution for the `explain` view: the 1-based source
/// number (an index into the numbered Sources list) of the file that set or
/// last-overrode each top-level knob, each profile, and each alias.
#[derive(Default)]
struct Provenance {
    knobs: HashMap<String, usize>,
    profiles: HashMap<String, usize>,
    aliases: HashMap<String, usize>,
}

/// Compute per-item provenance from the ordered config layers, reusing the
/// serialize-`Profile`-to-`toml::Table` presence trick from
/// `config show --sources` to know which top-level keys a file set.
///
/// Layers are walked low->high precedence (farther-from-cwd first), so a
/// later (closer) file overwrites the recorded number -- the closest-wins
/// rule the real merge applies. The attribution differs by item kind:
///
/// - top-level knobs merge per-key, so `[N]` is the closest file that set
///   THAT key.
/// - aliases are wholesale (closest file wins outright), so `[N]` is the
///   closest file defining the alias name.
/// - profiles merge field-by-field across files, so a clean single-file
///   attribution is not cheaply available; the compact choice is the
///   closest file that contributes ANY key to the profile.
fn compute_provenance(layers: &[(PathBuf, ConfigFile)]) -> Result<Provenance> {
    let mut prov = Provenance::default();
    for (i, (_, cfg)) in layers.iter().enumerate() {
        let n = i + 1;
        for key in profile_table(&cfg.defaults)?.keys() {
            prov.knobs.insert(key.clone(), n);
        }
        for name in cfg.profile.keys() {
            prov.profiles.insert(name.clone(), n);
        }
        for name in cfg.alias.keys() {
            prov.aliases.insert(name.clone(), n);
        }
    }
    Ok(prov)
}

/// Render a trailing source marker `[N]` (dim), or nothing when the item has
/// no attributed source.
fn source_tag(p: &Painter, num: Option<usize>) -> String {
    match num {
        Some(n) => format!(" {}", p.dim(&format!("[{n}]"))),
        None => String::new(),
    }
}

/// Build the full `config explain` narrative. Pure over its inputs (pool,
/// the auto-applied profile, the ordered config layers, the knob
/// descriptions, the color gate) so it is tested directly without a TTY or
/// environment.
fn render_explain(
    pool: &Pool,
    active: &Option<(String, &'static str)>,
    layers: &[(PathBuf, ConfigFile)],
    descs: &HashMap<String, String>,
    p: &Painter,
) -> Result<String> {
    let prov = compute_provenance(layers)?;
    let mut out = String::new();

    // Active profile.
    out.push_str(&p.header("Active profile"));
    out.push('\n');
    match active {
        Some((name, reason)) => {
            out.push_str(&format!(
                "  {} {}\n",
                p.key(name),
                p.dim(&format!("({reason}) -- see Profiles below"))
            ));
        }
        None => out.push_str(&format!("  {}\n", p.dim("none -- no profile auto-applies"))),
    }

    // Top-level defaults (always on).
    out.push('\n');
    out.push_str(&p.header("Top-level defaults (always on)"));
    out.push('\n');
    let defaults = profile_table(&pool.defaults)?;
    if defaults.is_empty() {
        out.push_str(&format!("  {}\n", p.dim("(none)")));
    } else {
        let guns = loaded_guns(&defaults);
        for (key, value) in &defaults {
            // Paint a loaded-gun knob yellow inline so risk catches the eye;
            // a safe knob keeps the cyan key style. The `[!] loaded gun`
            // marker below carries the same signal as text when color is off.
            let painted_key = if guns.contains(&key.as_str()) {
                p.warn(key)
            } else {
                p.key(key)
            };
            let mut line = format!("  {} = {}", painted_key, format_toml_value(value));
            if let Some(d) = desc_for(descs, key) {
                line.push_str(&format!("  {}", p.dim(&format!("-- {d}"))));
            }
            line.push_str(&source_tag(p, prov.knobs.get(key).copied()));
            out.push_str(&line);
            out.push('\n');
        }
        if !guns.is_empty() {
            out.push_str(&format!(
                "  {} {}\n",
                p.warn("[!] loaded gun at top level:"),
                p.warn(&guns.join(", "))
            ));
            out.push_str(&format!(
                "      {}\n",
                p.dim("task-scoped powers belong in a named [profile.NAME], not always-on")
            ));
        }
    }

    // Profiles (opt in with --profile NAME).
    out.push('\n');
    out.push_str(&p.header("Profiles (opt in with --profile NAME)"));
    out.push('\n');
    if pool.profiles.is_empty() {
        out.push_str(&format!("  {}\n", p.dim("(none)")));
    } else {
        let mut names: Vec<&String> = pool.profiles.keys().collect();
        names.sort();
        for name in names {
            let table = profile_table(&pool.profiles[name])?;
            out.push_str(&format!(
                "  {}{}  {}\n",
                p.key(name),
                source_tag(p, prov.profiles.get(name).copied()),
                posture_summary(&table, p)
            ));
            let guns = loaded_guns(&table);
            if !guns.is_empty() {
                out.push_str(&format!(
                    "      {} {}\n",
                    p.warn("[!] loaded gun:"),
                    p.warn(&guns.join(", "))
                ));
            }
        }
    }

    // Aliases (verbs).
    out.push('\n');
    out.push_str(&p.header("Aliases (verbs)"));
    out.push('\n');
    if pool.aliases.is_empty() {
        out.push_str(&format!("  {}\n", p.dim("(none)")));
    } else {
        let mut names: Vec<&String> = pool.aliases.keys().collect();
        names.sort();
        for name in names {
            let alias = &pool.aliases[name];
            out.push_str(&format!(
                "  {}{}  {}\n",
                p.key(&alias_invocation(name, alias)),
                source_tag(p, prov.aliases.get(name).copied()),
                p.dim(alias.description.as_deref().unwrap_or(""))
            ));
        }
    }

    // Sources (numbered; the [N] markers above point here).
    out.push('\n');
    out.push_str(&p.header("Sources (closest-to-cwd wins)"));
    out.push('\n');
    if layers.is_empty() {
        out.push_str(&format!("  {}\n", p.dim("(none)")));
    } else {
        for (i, (path, _)) in layers.iter().enumerate() {
            out.push_str(&format!(
                "  {} {}\n",
                p.dim(&format!("[{}]", i + 1)),
                p.dim(&path.display().to_string())
            ));
        }
        // Legend for the [N] markers, including the one inexact case.
        out.push_str(&format!(
            "  {}\n",
            p.dim("[N] marks the file that set each value above; for a profile it is the closest file contributing to it (profiles merge per-key across files).")
        ));
    }

    Ok(out)
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
        let p = init_prompt(Some("focus on PR review and a docs-build verb"), None);
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
        let p = init_prompt(None, None);
        assert!(p.contains("STARTER project"), "{p}");
        // No dangling "steer" sentence when none was given.
        assert!(!p.contains("The user added this steer"), "{p}");
    }

    #[test]
    fn init_prompt_steers_off_top_level_loaded_guns_and_plans() {
        let p = init_prompt(None, None);
        // Never put full_auto / anonymous worktree at the top level.
        assert!(p.contains("the TOP LEVEL"), "{p}");
        assert!(p.contains("full_auto"), "{p}");
        assert!(p.contains("worktree = true"), "{p}");
        assert!(p.contains("NAME the loaded guns"), "{p}");
        // Reusable verbs, not a one-shot plan.
        assert!(p.contains("REUSABLE verbs"), "{p}");
        assert!(p.contains("phase0"), "{p}");
        assert!(p.contains("plan's home is GitHub issues"), "{p}");
        // Per-layer purpose block.
        assert!(p.contains("AMBIENT, always-on knobs"), "{p}");
        assert!(p.contains("named, opt-in-able MODE"), "{p}");
        assert!(p.contains("profile PLUS a prompt template"), "{p}");
        // Alias-only keys must not be steered into a [profile.NAME] table.
        assert!(
            p.contains("they are alias keys and a `[profile.NAME]` table will REJECT them"),
            "{p}"
        );
    }

    #[test]
    fn init_prompt_injects_inherited_pool_and_delta_instruction() {
        let p = init_prompt(Some("PR review"), Some("readonly = true\n"));
        // The inherited config text is embedded verbatim.
        assert!(p.contains("readonly = true"), "{p}");
        assert!(p.contains("ALREADY inherited by this project"), "{p}");
        // The delta instruction is present.
        assert!(p.contains("project-specific DELTA"), "{p}");
        assert!(
            p.contains("do NOT re-state keys that are already inherited"),
            "{p}"
        );
    }

    #[test]
    fn init_prompt_omits_inherited_section_when_none() {
        let p = init_prompt(None, None);
        // No dangling inherited-config heading when nothing is inherited.
        assert!(!p.contains("ALREADY inherited by this project"), "{p}");
        assert!(!p.contains("project-specific DELTA"), "{p}");
    }

    #[test]
    fn render_merged_pool_round_trips() {
        // A pool with all four section kinds renders to TOML that parses
        // back through roba's real per-file deserializer, with values
        // intact -- the byte-clean, re-parseable stdout contract.
        let mut pool = Pool::default();
        pool.defaults.readonly = Some(true);
        pool.defaults
            .vars
            .insert("TEAM".to_string(), "core".to_string());
        pool.profiles.insert(
            "worker".to_string(),
            Profile {
                full_auto: Some(true),
                max_turns: Some(80),
                ..Default::default()
            },
        );
        pool.aliases.insert(
            "review".to_string(),
            Alias {
                description: Some("review a PR".to_string()),
                ..Default::default()
            },
        );
        pool.sessions.insert(
            "meta".to_string(),
            "11111111-1111-4111-8111-111111111111".to_string(),
        );

        let rendered = render_merged_pool(&pool).unwrap();
        let cfg = crate::profile::pool::parse_config_str(&rendered)
            .unwrap_or_else(|e| panic!("merged pool did not re-parse: {e:#}\n---\n{rendered}"));

        assert_eq!(cfg.defaults.readonly, Some(true));
        assert_eq!(
            cfg.defaults.vars.get("TEAM").map(String::as_str),
            Some("core")
        );
        assert_eq!(cfg.profile["worker"].full_auto, Some(true));
        assert_eq!(cfg.profile["worker"].max_turns, Some(80));
        assert_eq!(
            cfg.alias["review"].description.as_deref(),
            Some("review a PR")
        );
        assert_eq!(
            cfg.session.get("meta").map(String::as_str),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }

    #[test]
    fn render_merged_pool_empty_is_blank() {
        // An empty pool -> empty stdout (no header, no stray newline).
        let pool = Pool::default();
        assert_eq!(render_merged_pool(&pool).unwrap(), "");
    }

    // -- Effective view / per-key provenance (#330 part 2) -----------------

    fn file_layer(path: &str, defaults: Profile) -> (PathBuf, ConfigFile) {
        (
            PathBuf::from(path),
            ConfigFile {
                defaults,
                ..Default::default()
            },
        )
    }

    #[test]
    fn compute_effective_highest_layer_wins() {
        // farther file: readonly + model; closer file overrides model; the
        // auto-applied profile sets max_turns; env sets worktree.
        let layers = vec![
            file_layer(
                "/repo/roba.toml",
                Profile {
                    readonly: Some(true),
                    model: Some("sonnet".into()),
                    ..Default::default()
                },
            ),
            file_layer(
                "/repo/sub/roba.toml",
                Profile {
                    model: Some("opus".into()),
                    ..Default::default()
                },
            ),
        ];
        let mut pool = Pool::default();
        pool.profiles.insert(
            "worker".to_string(),
            Profile {
                max_turns: Some(80),
                ..Default::default()
            },
        );
        let active = Some(("worker".to_string(), "auto-applied"));
        let env = HashMap::from([("ROBA_WORKTREE".to_string(), "1".to_string())]);

        let (eff, unattributed) = compute_effective(&layers, &active, &pool, &env).unwrap();

        // readonly: only the farther file set it.
        let (v, src) = &eff["readonly"];
        assert_eq!(v, &Value::Boolean(true));
        assert!(src.contains("/repo/roba.toml"), "{src}");
        // model: closer file wins over farther.
        let (v, src) = &eff["model"];
        assert_eq!(v, &Value::String("opus".into()));
        assert!(src.contains("/repo/sub/roba.toml"), "{src}");
        // max_turns: from the auto-applied profile.
        let (v, src) = &eff["max_turns"];
        assert_eq!(v, &Value::Integer(80));
        assert_eq!(src, "[profile.worker]");
        // worktree: env wins (highest layer).
        let (v, src) = &eff["worktree"];
        assert_eq!(v, &Value::Boolean(true));
        assert_eq!(src, "env (ROBA_WORKTREE)");
        assert!(unattributed.is_empty());
    }

    #[test]
    fn compute_effective_profile_overrides_top_level() {
        // A key set by BOTH a top-level file and the active profile is won
        // by the profile (it overlays on top of top-level keys).
        let layers = vec![file_layer(
            "/repo/roba.toml",
            Profile {
                model: Some("sonnet".into()),
                ..Default::default()
            },
        )];
        let mut pool = Pool::default();
        pool.profiles.insert(
            "p".to_string(),
            Profile {
                model: Some("opus".into()),
                ..Default::default()
            },
        );
        let active = Some(("p".to_string(), "auto-applied"));
        let (eff, _) = compute_effective(&layers, &active, &pool, &HashMap::new()).unwrap();
        let (v, src) = &eff["model"];
        assert_eq!(v, &Value::String("opus".into()));
        assert_eq!(src, "[profile.p]");
    }

    #[test]
    fn compute_effective_only_shows_set_keys() {
        // No layer sets `writable`, so it never appears (no built-in-default
        // row for unset knobs).
        let layers = vec![file_layer(
            "/repo/roba.toml",
            Profile {
                readonly: Some(true),
                ..Default::default()
            },
        )];
        let (eff, _) =
            compute_effective(&layers, &None, &Pool::default(), &HashMap::new()).unwrap();
        assert!(eff.contains_key("readonly"));
        assert!(!eff.contains_key("writable"));
    }

    #[test]
    fn env_entries_value_semantics() {
        let mut un = Vec::new();
        let env = HashMap::from([
            ("ROBA_READONLY".to_string(), "yes".to_string()),
            ("ROBA_GIT_DIFF".to_string(), "garbage".to_string()), // falsy/garbage -> no-op
            ("ROBA_ALLOW_TOOL".to_string(), "Edit, Write".to_string()),
            ("ROBA_MAX_TURNS".to_string(), "40".to_string()),
            ("ROBA_MODEL".to_string(), "opus".to_string()),
        ]);
        let entries = env_entries(&env, &mut un);
        let by_key: HashMap<&str, &Value> =
            entries.iter().map(|(k, v, _)| (k.as_str(), v)).collect();
        assert_eq!(by_key["readonly"], &Value::Boolean(true));
        assert!(!by_key.contains_key("git_diff")); // garbage bool ignored
        assert_eq!(
            by_key["allow_tool"],
            &Value::Array(vec![
                Value::String("Edit".into()),
                Value::String("Write".into())
            ])
        );
        assert_eq!(by_key["max_turns"], &Value::Integer(40));
        assert_eq!(by_key["model"], &Value::String("opus".into()));
        assert!(un.is_empty());
    }

    #[test]
    fn env_entries_unattributed_choice_and_vars() {
        let mut un = Vec::new();
        let env = HashMap::from([
            ("ROBA_EFFORT".to_string(), "ludicrous".to_string()), // not a known effort
            ("ROBA_VAR_TICKET".to_string(), "ABC-123".to_string()),
        ]);
        let entries = env_entries(&env, &mut un);
        assert!(entries.iter().all(|(k, _, _)| k != "effort"));
        assert!(un.iter().any(|s| s.contains("ROBA_EFFORT")), "{un:?}");
        assert!(un.iter().any(|s| s.contains("ROBA_VAR_")), "{un:?}");
    }

    #[test]
    fn render_key_line_formats_scalars_and_arrays() {
        assert_eq!(
            render_key_line("model", &Value::String("opus".into()), "[profile.worker]").unwrap(),
            "model = \"opus\"  # [profile.worker]\n"
        );
        assert_eq!(
            render_key_line("full_auto", &Value::Boolean(true), "env (ROBA_FULL_AUTO)").unwrap(),
            "full_auto = true  # env (ROBA_FULL_AUTO)\n"
        );
        assert_eq!(
            render_key_line(
                "allow_tool",
                &Value::Array(vec![Value::String("Edit".into())]),
                "/repo/roba.toml (top-level)"
            )
            .unwrap(),
            "allow_tool = [\"Edit\"]  # /repo/roba.toml (top-level)\n"
        );
    }

    // -- config explain (#357) ---------------------------------------------

    #[test]
    fn knob_descriptions_draw_from_cli_help() {
        // The per-knob "what it does" text is sourced from the cli.rs doc
        // comments via clap, so explain and --help cannot drift. A handful
        // of stable knobs must resolve to non-empty descriptions.
        let descs = knob_descriptions();
        for key in ["model", "readonly", "max_turns", "full_auto"] {
            assert!(
                desc_for(&descs, key).is_some_and(|d| !d.is_empty()),
                "no description for `{key}` (have {} keys)",
                descs.len()
            );
        }
    }

    #[test]
    fn desc_for_bridges_continue_rename() {
        // The TOML key `continue` maps to the `continue_session` arg id.
        let descs = knob_descriptions();
        assert!(desc_for(&descs, "continue").is_some());
    }

    #[test]
    fn format_toml_value_renders_scalars_and_arrays() {
        assert_eq!(format_toml_value(&Value::String("opus".into())), "\"opus\"");
        assert_eq!(format_toml_value(&Value::Boolean(true)), "true");
        assert_eq!(format_toml_value(&Value::Integer(80)), "80");
        assert_eq!(
            format_toml_value(&Value::Array(vec![
                Value::String("Edit".into()),
                Value::String("Write".into())
            ])),
            "[\"Edit\", \"Write\"]"
        );
    }

    #[test]
    fn loaded_guns_flags_full_auto_and_worktree() {
        let p = Profile {
            full_auto: Some(true),
            worktree: Some(crate::profile::WorktreeSetting::Enabled(true)),
            ..Default::default()
        };
        let guns = loaded_guns(&profile_table(&p).unwrap());
        assert!(guns.contains(&"full_auto"), "{guns:?}");
        assert!(guns.contains(&"worktree"), "{guns:?}");

        // readonly-only profile: no loaded guns.
        let safe = Profile {
            readonly: Some(true),
            ..Default::default()
        };
        assert!(loaded_guns(&profile_table(&safe).unwrap()).is_empty());

        // worktree = false explicitly DISABLES it, so it is not a loaded gun.
        let no_wt = Profile {
            worktree: Some(crate::profile::WorktreeSetting::Enabled(false)),
            ..Default::default()
        };
        assert!(
            !loaded_guns(&profile_table(&no_wt).unwrap()).contains(&"worktree"),
            "worktree = false must not be flagged"
        );

        // a NAMED worktree is still a loaded gun.
        let named = Profile {
            worktree: Some(crate::profile::WorktreeSetting::Named("wt".into())),
            ..Default::default()
        };
        assert!(loaded_guns(&profile_table(&named).unwrap()).contains(&"worktree"));
    }

    #[test]
    fn posture_summary_is_compact() {
        let p = Profile {
            readonly: Some(true),
            max_turns: Some(80),
            ..Default::default()
        };
        let painter = Painter { color: false };
        let s = posture_summary(&profile_table(&p).unwrap(), &painter);
        assert!(s.contains("readonly"), "{s}");
        assert!(s.contains("max_turns=80"), "{s}");
        // An empty profile reads as no overrides.
        assert_eq!(
            posture_summary(&profile_table(&Profile::default()).unwrap(), &painter),
            "(no overrides)"
        );
    }

    #[test]
    fn render_explain_paints_loaded_guns_yellow() {
        let mut pool = Pool::default();
        pool.defaults.readonly = Some(true); // safe -- keeps the cyan key style
        pool.defaults.full_auto = Some(true); // a top-level loaded gun
        pool.profiles.insert(
            "worker".to_string(),
            Profile {
                full_auto: Some(true),
                max_turns: Some(80),
                ..Default::default()
            },
        );

        // Color ON: the loaded-gun items wear the bold-yellow span, inline,
        // at both the top level and inside the profile's posture summary.
        let on = Painter { color: true };
        let colored = render_explain(&pool, &None, &[], &knob_descriptions(), &on).unwrap();
        assert!(
            colored.contains("\x1b[1;33mfull_auto\x1b[0m"),
            "loaded-gun item not painted yellow:\n{colored:?}"
        );
        // A safe knob is NOT yellow (it wears the cyan key style instead).
        assert!(
            !colored.contains("\x1b[1;33mreadonly"),
            "safe knob wrongly painted yellow:\n{colored:?}"
        );

        // Color OFF: no ANSI escapes, but the text marker survives -- the risk
        // signal never depends on color alone.
        let off = Painter { color: false };
        let plain = render_explain(&pool, &None, &[], &knob_descriptions(), &off).unwrap();
        assert!(
            !plain.contains('\x1b'),
            "plain mode leaked ANSI:\n{plain:?}"
        );
        assert!(plain.contains("loaded gun at top level"), "{plain}");
        assert!(plain.contains("[!] loaded gun:"), "{plain}");
    }

    #[test]
    fn alias_invocation_forms() {
        let with_args = Alias {
            args: vec!["pr".into()],
            template: Some("review ${pr}".into()),
            ..Default::default()
        };
        assert_eq!(alias_invocation("review", &with_args), "roba review <pr>");

        let template_only = Alias {
            template: Some("do the thing".into()),
            ..Default::default()
        };
        assert_eq!(alias_invocation("go", &template_only), "roba go ...");

        let shortcut = Alias::default();
        assert_eq!(alias_invocation("q", &shortcut), "roba q <prompt...>");
    }

    #[test]
    fn render_explain_plain_lays_out_every_section() {
        let mut pool = Pool::default();
        pool.defaults.readonly = Some(true);
        pool.defaults.full_auto = Some(true); // a top-level loaded gun
        pool.profiles.insert(
            "worker".to_string(),
            Profile {
                full_auto: Some(true),
                max_turns: Some(80),
                ..Default::default()
            },
        );
        pool.aliases.insert(
            "review".to_string(),
            Alias {
                description: Some("review a PR".to_string()),
                args: vec!["pr".into()],
                template: Some("review ${pr}".into()),
                ..Default::default()
            },
        );
        pool.sources.push(PathBuf::from("/repo/roba.toml"));
        let active = Some(("worker".to_string(), "auto-applied"));

        // One layer carrying every item the pool displays, so provenance
        // attributes each value to `[1]`.
        let layers = vec![(
            PathBuf::from("/repo/roba.toml"),
            ConfigFile {
                defaults: Profile {
                    readonly: Some(true),
                    full_auto: Some(true),
                    ..Default::default()
                },
                profile: HashMap::from([(
                    "worker".to_string(),
                    Profile {
                        full_auto: Some(true),
                        max_turns: Some(80),
                        ..Default::default()
                    },
                )]),
                alias: HashMap::from([(
                    "review".to_string(),
                    Alias {
                        description: Some("review a PR".to_string()),
                        args: vec!["pr".into()],
                        template: Some("review ${pr}".into()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            },
        )];

        let p = Painter { color: false };
        let text = render_explain(&pool, &active, &layers, &knob_descriptions(), &p).unwrap();

        // Every section header is present.
        assert!(text.contains("Active profile"), "{text}");
        assert!(text.contains("Top-level defaults (always on)"), "{text}");
        assert!(
            text.contains("Profiles (opt in with --profile NAME)"),
            "{text}"
        );
        assert!(text.contains("Aliases (verbs)"), "{text}");
        assert!(text.contains("Sources (closest-to-cwd wins)"), "{text}");

        // Active profile is named with its reason.
        assert!(text.contains("worker (auto-applied)"), "{text}");
        // Top-level knob carries its cli.rs description.
        assert!(text.contains("readonly = true"), "{text}");
        assert!(
            text.contains("-- "),
            "no knob description rendered:\n{text}"
        );
        // Loaded gun is flagged at both the top level and the profile.
        assert!(text.contains("loaded gun at top level"), "{text}");
        assert!(text.contains("[!] loaded gun:"), "{text}");
        // Alias invocation form + description.
        assert!(text.contains("roba review <pr>"), "{text}");
        assert!(text.contains("review a PR"), "{text}");
        // Source file listed, numbered.
        assert!(text.contains("[1] /repo/roba.toml"), "{text}");
        // Every rendered item carries its source number.
        assert!(text.contains("readonly = true"), "{text}");
        assert!(text.contains("worker [1]"), "{text}");
        assert!(text.contains("roba review <pr> [1]"), "{text}");
        // The legend explaining the [N] markers is present.
        assert!(text.contains("[N] marks the file"), "{text}");
        // Plain mode emits no ANSI escapes.
        assert!(!text.contains('\x1b'), "plain mode leaked ANSI:\n{text}");
    }

    #[test]
    fn render_explain_empty_pool_shows_nones() {
        let pool = Pool::default();
        let p = Painter { color: false };
        let text = render_explain(&pool, &None, &[], &knob_descriptions(), &p).unwrap();
        assert!(text.contains("none -- no profile auto-applies"), "{text}");
        // Each empty section degrades to a `(none)` line, not a crash.
        assert_eq!(text.matches("(none)").count(), 4, "{text}");
    }

    fn explain_layer(path: &str, cfg: ConfigFile) -> (PathBuf, ConfigFile) {
        (PathBuf::from(path), cfg)
    }

    #[test]
    fn compute_provenance_attributes_each_item_kind() {
        // [1] farther file sets readonly + model and defines the `worker`
        //     profile + `review` alias.
        // [2] closer file overrides model, contributes another key to the
        //     `worker` profile, and redefines `review`.
        let layers = vec![
            explain_layer(
                "/repo/roba.toml",
                ConfigFile {
                    defaults: Profile {
                        readonly: Some(true),
                        model: Some("sonnet".into()),
                        ..Default::default()
                    },
                    profile: HashMap::from([(
                        "worker".to_string(),
                        Profile {
                            full_auto: Some(true),
                            ..Default::default()
                        },
                    )]),
                    alias: HashMap::from([("review".to_string(), Alias::default())]),
                    ..Default::default()
                },
            ),
            explain_layer(
                "/repo/sub/roba.toml",
                ConfigFile {
                    defaults: Profile {
                        model: Some("opus".into()),
                        ..Default::default()
                    },
                    profile: HashMap::from([(
                        "worker".to_string(),
                        Profile {
                            max_turns: Some(80),
                            ..Default::default()
                        },
                    )]),
                    alias: HashMap::from([("review".to_string(), Alias::default())]),
                    ..Default::default()
                },
            ),
        ];

        let prov = compute_provenance(&layers).unwrap();
        // Per-key knob attribution: readonly only the farther file set it;
        // model the closer file last-overrode.
        assert_eq!(prov.knobs.get("readonly").copied(), Some(1));
        assert_eq!(prov.knobs.get("model").copied(), Some(2));
        // Profiles merge per-key; the compact choice is the closest file
        // contributing any key -> [2].
        assert_eq!(prov.profiles.get("worker").copied(), Some(2));
        // Aliases are wholesale; the closest file defining it wins -> [2].
        assert_eq!(prov.aliases.get("review").copied(), Some(2));
        // Unset items have no attribution.
        assert!(!prov.knobs.contains_key("writable"));
    }

    #[test]
    fn render_explain_numbers_multiple_sources() {
        // Two files in the pool render as a numbered Sources list.
        let mut pool = Pool::default();
        pool.defaults.readonly = Some(true);
        let layers = vec![
            explain_layer(
                "/repo/roba.toml",
                ConfigFile {
                    defaults: Profile {
                        readonly: Some(true),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            explain_layer("/repo/sub/roba.toml", ConfigFile::default()),
        ];
        let p = Painter { color: false };
        let text = render_explain(&pool, &None, &layers, &knob_descriptions(), &p).unwrap();
        assert!(text.contains("[1] /repo/roba.toml"), "{text}");
        assert!(text.contains("[2] /repo/sub/roba.toml"), "{text}");
        // The top-level knob is attributed to the file that set it.
        assert!(text.contains("readonly = true"), "{text}");
    }

    #[test]
    fn painter_color_wraps_and_resets() {
        let p = Painter { color: true };
        let h = p.header("X");
        assert!(h.starts_with('\x1b') && h.ends_with("\x1b[0m"), "{h}");
        // Plain painter leaves text untouched.
        let plain = Painter { color: false };
        assert_eq!(plain.header("X"), "X");
    }
}
