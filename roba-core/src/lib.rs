//! roba-core: the clap-free, side-effect-free run engine.
//!
//! This crate is roba's resolve-free core, split out from the CLI (#416). It
//! owns two things:
//!
//! - [`engine`] -- the config-and-run seam. [`engine::run`] takes a
//!   [`engine::Config`] and returns a [`claude_wrapper::types::QueryResult`]:
//!   no clap, no stdout/stderr, no `process::exit`, no TTY. It is the down
//!   payment on `serve` (#142), where a request maps to a `Config` and calls
//!   `engine::run`.
//! - [`session`] -- [`session::apply_session`], the single `Config ->
//!   QueryCommand` mapper the engine feeds, plus the permission/notice
//!   composition it consumes.
//!
//! The `roba` binary depends on this crate: its `run_ask` resolves
//! flags/profiles/prompt into a `Config` (`build_config`) and renders the
//! result around [`engine::run`]'s seam, so there is one flag->command mapper
//! and no second copy to drift. Prompt composition, profile/env layering, live
//! streaming display, output formatting, and exit-code classification stay in
//! the CLI crate.

pub mod engine;
pub mod session;
