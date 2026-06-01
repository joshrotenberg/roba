//! Config system for `roba`.
//!
//! A `roba.toml` file holds two kinds of content:
//!
//! - **Top-level keys** -- defaults applied to every call in this dir
//! - **`[profile.NAME]` tables** -- named overlays opted into with
//!   `--profile NAME` or `ROBA_PROFILE=NAME`
//!
//! # Resolution
//!
//! For any setting, the highest layer that defines it wins:
//!
//! 1. CLI flag
//! 2. `[profile.NAME]` overlay (when activated)
//! 3. Top-level keys in `roba.toml` files
//! 4. roba's built-in defaults
//! 5. claude's defaults
//!
//! (The `ROBA_<PARAM>` env-var override layer slots in between CLI
//! and the profile overlay; it lands in a follow-up PR.)
//!
//! # File discovery
//!
//! 1. **User-level:** `$XDG_CONFIG_HOME/roba.toml` or
//!    `~/.config/roba.toml`
//! 2. **Project chain:** every ancestor `roba.toml` walking up from
//!    cwd to the git root (or `~` if no git root); farther-from-cwd
//!    files are loaded first so closer files override on conflict
//!
//! `ROBA_PROFILES_FILE` (point-at-an-extra-file env var) was retired
//! in favour of the per-knob `ROBA_<PARAM>` override layer; see
//! [`crate::env`].
//!
//! # Merge semantics
//!
//! When the same key appears in multiple files (top-level or inside
//! a `[profile.NAME]` of the same name):
//!
//! - Scalars: closer-to-cwd file wins.
//! - Lists: concat. Closer-file items appended after farther ones.
//! - Maps (vars): per-key merge with closer winning on key conflicts.
//!
//! CLI flags then override the merged result via [`merge_into_args`].
//!
//! # Module layout
//!
//! - [`types`]: the [`Profile`] / [`ConfigFile`] / [`Pool`] data types
//! - [`pool`]: file discovery + loading into a merged [`Pool`]
//! - [`mod@resolve`]: picking the effective profile and merging it into args
//! - [`cmd`]: the `roba profile <action>` subcommand runners

pub mod cmd;
pub mod pool;
pub mod resolve;
pub mod types;

pub use cmd::{STARTER_CONFIG_TOML, run};
pub use pool::{discover_project_configs, load_pool, load_pool_from, user_config_path};
pub use resolve::{merge_into_args, profile_source_label, resolve, select_profile_name};
pub use types::{ConfigFile, ContinueSetting, Pool, Profile, WorktreeSetting};

use std::path::PathBuf;

/// Resolve the user's home directory from `HOME` (Unix) or
/// `USERPROFILE` (Windows). Shared by [`pool::user_config_path`] and
/// [`resolve`]'s `~/`-expansion.
pub(crate) fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("USERPROFILE")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    None
}
