//! Shared test fixtures: synthetic "pretend project" builder.
//!
//! A scenario test opts in with `mod common;` and builds an isolated
//! project on disk -- a `.git` marker (so the config walk-up stops there),
//! an optional project `roba.toml`, an optional user-config-layer
//! `roba.toml`, and arbitrary extra files -- then runs the roba binary with
//! `HOME` + `XDG_CONFIG_HOME` pinned to the fixture so the real user config
//! never leaks in. Mechanical only: nothing here invokes claude.
//!
//! Not every test binary uses every helper, so the crate would otherwise
//! warn on the unused ones.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

/// Run `roba` against `dir` via `-C`, defaulting to the haiku model.
/// Tests that need a specific model can append `--model <id>` later;
/// clap's last-occurrence-wins semantics applies.
pub fn roba_in(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("roba").expect("cargo-built roba binary");
    cmd.args([
        "-C",
        dir.to_str().expect("utf-8 tempdir path"),
        "--model",
        "haiku",
    ]);
    cmd
}

pub fn fresh_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("create test tempdir")
}

/// A synthetic project on disk with an isolated config environment.
///
/// Holds the backing [`TempDir`]s so they outlive the test; dropping the
/// `Project` removes both directories.
pub struct Project {
    _root: TempDir,
    _config_home: TempDir,
    /// The project directory (holds `.git`, an optional `roba.toml`, files).
    pub root: PathBuf,
    /// `$XDG_CONFIG_HOME` -- the user-config layer (holds an optional
    /// `roba.toml`).
    pub config_home: PathBuf,
}

/// Start building a synthetic project.
pub fn project() -> ProjectBuilder {
    ProjectBuilder::default()
}

#[derive(Default)]
pub struct ProjectBuilder {
    user_config: Option<String>,
    project_toml: Option<String>,
    files: Vec<(String, String)>,
    no_git: bool,
}

impl ProjectBuilder {
    /// Skip the `.git` marker so the project is NOT a git repository.
    ///
    /// The builder normally drops an empty `.git` dir so the config walk-up
    /// stops at the fixture. A few scenarios need the opposite -- the
    /// `-w`/worktree preflight bails when cwd is not a git repo (#327) -- so
    /// this opts out. (A bare temp dir under `/tmp` or `/var/folders` has no
    /// git ancestor, so the preflight sees a non-repo, the same as the
    /// `tests/cli.rs` proof.)
    pub fn no_git(mut self) -> Self {
        self.no_git = true;
        self
    }
    /// Contents of `$XDG_CONFIG_HOME/roba.toml` (the user-config layer). Not
    /// calling this leaves the user layer empty.
    pub fn user_config(mut self, toml: &str) -> Self {
        self.user_config = Some(toml.to_string());
        self
    }

    /// Contents of `<root>/roba.toml` (the project layer). Not calling this
    /// leaves the project layer empty.
    pub fn project_toml(mut self, toml: &str) -> Self {
        self.project_toml = Some(toml.to_string());
        self
    }

    /// Write an arbitrary file under the project root (parent dirs created).
    pub fn file(mut self, rel: &str, content: &str) -> Self {
        self.files.push((rel.to_string(), content.to_string()));
        self
    }

    /// Materialize the fixture on disk.
    pub fn build(self) -> Project {
        let root = tempfile::tempdir().expect("project root tempdir");
        let config_home = tempfile::tempdir().expect("config home tempdir");

        // The `.git` marker stops `discover_project_configs` walking up past
        // the fixture, so the config pool is deterministic. An empty dir is
        // enough -- no real `git init` required. `no_git()` opts out for the
        // scenarios that need a non-repo cwd (the worktree preflight, #327).
        if !self.no_git {
            std::fs::create_dir_all(root.path().join(".git")).expect("mkdir .git");
        }

        if let Some(toml) = &self.user_config {
            std::fs::write(config_home.path().join("roba.toml"), toml).expect("write user config");
        }
        if let Some(toml) = &self.project_toml {
            std::fs::write(root.path().join("roba.toml"), toml).expect("write project roba.toml");
        }
        for (rel, content) in &self.files {
            let p = root.path().join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("mkdir for fixture file");
            }
            std::fs::write(&p, content).expect("write fixture file");
        }

        let root_path = root.path().to_path_buf();
        let config_home_path = config_home.path().to_path_buf();
        Project {
            _root: root,
            _config_home: config_home,
            root: root_path,
            config_home: config_home_path,
        }
    }
}

impl Project {
    /// An [`assert_cmd::Command`] for the roba binary, scoped to this
    /// fixture: cwd = the project root (via `-C`, which roba applies to the
    /// process cwd before dispatch), `HOME` + `XDG_CONFIG_HOME` pinned to
    /// isolated temps so no real user config leaks in. `PATH` is inherited
    /// so the binary's own deps resolve; these scenarios never spawn claude.
    ///
    /// `ROBA_PROFILE` is cleared so an ambient selection var can't change
    /// which profile auto-applies; tests set or clear other `ROBA_*` vars
    /// as they need.
    pub fn roba(&self) -> Command {
        let mut cmd = Command::cargo_bin("roba").expect("cargo-built roba binary");
        cmd.arg("-C")
            .arg(&self.root)
            .env("HOME", &self.config_home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env_remove("ROBA_PROFILE");
        cmd
    }
}
