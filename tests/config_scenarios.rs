//! Mechanical "pretend project" scenarios over `roba config show` -- the
//! free, CI-run regression tier for the config/worktree footgun cluster
//! (#327-#330). `config show` is pure inspection: no claude is invoked.
//!
//! Each scenario builds an isolated synthetic project via the shared
//! fixture builder, then asserts the EXACT stdout/stderr `config show`
//! emits. Read `src/config.rs` (`run_show` / `render_merged_pool` /
//! `run_show_sources`) for the formats these pin.

mod common;

use common::*;
use predicates::prelude::*;

// ---------------------------------------------------------------------------
// A1 -- layered config resolution: project file overrides the user layer,
// a `[profile.default]` auto-applies, and `--sources` attributes each key.
// ---------------------------------------------------------------------------

/// The A1 fixture: a user-config layer (top-level `model = "haiku"` plus a
/// `[profile.default]` with `max_turns = 80`) and a project layer that
/// overrides `model = "opus"`.
fn a1_project() -> Project {
    project()
        .user_config("model = \"haiku\"\n\n[profile.default]\nmax_turns = 80\n")
        .project_toml("model = \"opus\"\n")
        .build()
}

#[test]
fn a1_show_merges_layers_to_stdout_header_to_stderr() {
    // The merged view: the project model wins (`opus`), the user-layer
    // `[profile.default]` (and its `max_turns`) survives the merge. The body
    // is byte-clean on STDOUT; the active-profile + sources header is
    // METADATA on STDERR (stream routing, principle #2).
    a1_project()
        .roba()
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("model = \"opus\""))
        .stdout(predicate::str::contains("[profile.default]"))
        .stdout(predicate::str::contains("max_turns = 80"))
        // The header must not leak into the pipeable body.
        .stdout(predicate::str::contains("active profile:").not())
        .stderr(predicate::str::contains("active profile: default"))
        .stderr(predicate::str::contains("sources:"));
}

#[test]
fn a1_sources_attributes_each_key_to_its_winning_layer() {
    // The effective/provenance view: `model` resolves to the closer project
    // file's value (`opus` beats the user file's `haiku`), and `max_turns`
    // is attributed to the auto-applied `[profile.default]`.
    let proj = a1_project();
    let assert = proj
        .roba()
        .args(["config", "show", "--sources"])
        .env_remove("ROBA_MODEL")
        .env_remove("ROBA_MAX_TURNS")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    // model: closer project file won (value `opus`), attributed to a
    // top-level file layer -- and specifically the project file path.
    let model = stdout
        .lines()
        .find(|l| l.starts_with("model ="))
        .unwrap_or_else(|| panic!("no model line in:\n{stdout}"));
    assert!(model.contains("model = \"opus\""), "{model}");
    assert!(model.contains("(top-level)"), "{model}");
    assert!(
        model.contains(&proj.root.display().to_string()),
        "model should attribute to the project file: {model}"
    );

    // max_turns: from the auto-applied profile.
    assert!(
        stdout.contains("max_turns = 80  # [profile.default]"),
        "got:\n{stdout}"
    );
}

#[test]
fn a1_sources_single_key_prints_only_that_key() {
    a1_project()
        .roba()
        .args(["config", "show", "--sources", "model"])
        .env_remove("ROBA_MODEL")
        .assert()
        .success()
        .stdout(predicate::str::contains("model = \"opus\""))
        // Only the requested key prints -- no other effective key leaks.
        .stdout(predicate::str::contains("max_turns").not());
}

#[test]
fn a1_sources_env_layer_wins_and_is_attributed() {
    // `ROBA_MODEL` is the highest config layer, so it overrides the project
    // file and is attributed to `env (ROBA_MODEL)`.
    a1_project()
        .roba()
        .args(["config", "show", "--sources", "model"])
        .env("ROBA_MODEL", "sonnet")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "model = \"sonnet\"  # env (ROBA_MODEL)",
        ));
}

// ---------------------------------------------------------------------------
// A2 -- the orario footgun, made visible: a top-level `worktree = true` in
// the project file is exactly the shape that silently minted anonymous
// worktrees and defeated `-c`. `config show --sources worktree` surfaces it
// instantly (it took manual JSONL spelunking to find in the field).
// ---------------------------------------------------------------------------

#[test]
fn a2_sources_surfaces_top_level_worktree_footgun() {
    let proj = project().project_toml("worktree = true\n").build();
    let assert = proj
        .roba()
        .args(["config", "show", "--sources", "worktree"])
        .env_remove("ROBA_WORKTREE")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    let worktree = stdout
        .lines()
        .find(|l| l.starts_with("worktree ="))
        .unwrap_or_else(|| panic!("no worktree line in:\n{stdout}"));
    assert!(worktree.contains("worktree = true"), "{worktree}");
    assert!(worktree.contains("(top-level)"), "{worktree}");
    assert!(
        worktree.contains(&proj.root.display().to_string()),
        "worktree should attribute to the project file: {worktree}"
    );
}
