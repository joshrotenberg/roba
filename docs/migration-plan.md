# Migration plan: roba → roba, claude-wrapper workspace → own repo

One-time work. Tracked here so we don't lose pieces during the move.
Delete this file once the migration is done.

## Decisions already made

- **New name:** `roba` (Venetian for "stuff / things"). Crate, bin,
  repo, config paths all use this.
- **Dedicated repo.** Out of the claude-wrapper Cargo workspace.
- **claude-wrapper dep:** switch from `path = "../claude-wrapper"`
  to `version = "0.x"` from crates.io. claude-wrapper 0.9 already
  has every wrapper-side feature roba uses (history pagination /
  aiTitle fix, skills / settings / commands modules, duplex
  permission builders -- the salvage round earlier this session).
- **History preservation:** TBD. Options below.

## Pre-move checklist (in this repo)

- [ ] Confirm claude-wrapper's published version has everything
      roba needs. Spot check by building `roba` against the *crates.io*
      version of claude-wrapper instead of the path dep -- temporarily
      swap in `Cargo.toml`, `cargo build -p roba`, confirm clean, then
      revert.
- [ ] Run the full `roba` test suite one more time (unit +
      mechanical CLI). Live tests too if you want to bank a green
      run.
- [ ] Note current roba commit SHA for reference in the new repo's
      first commit message ("forked from claude-wrapper@SHA").

## The move

### Pick a history strategy

| Strategy | Pro | Con |
|---|---|---|
| **Fresh start** | Clean log starting at `Initial roba.` Faster to set up. | Loses ~45 commits of design history and rationale. |
| **`git filter-repo --subdirectory-filter`** | Preserves the per-commit history with full messages. The "we built this incrementally" trail stays intact. | Tooling adds friction (filter-repo isn't in core git); some commits touched both claude-wrapper and roba, those need cleanup. |
| **Hybrid: squash + curated CHANGELOG** | Single "Initial roba" commit, but the CHANGELOG.md has the milestones. Tells the story without preserving line-by-line. | Manual write-up of the CHANGELOG entries. |

Lean: **filter-repo**. Worth the half-hour setup -- the commit
messages from this session are themselves design docs.

### Steps (assuming filter-repo)

1. **Create new GitHub repo** `joshrotenberg/roba`. Don't init
   with README / LICENSE / .gitignore; we're pushing existing
   content.
2. **Clone claude-wrapper to a working dir** specifically for
   surgery: `git clone --no-local <claude-wrapper> roba-extract`.
3. **Filter to roba-relevant paths** (`crates/roba/`,
   `docs/roba/`, root files we want):
   ```bash
   cd roba-extract
   git filter-repo \
     --path crates/roba \
     --path docs/roba \
     --path-rename crates/roba:. \
     --path-rename docs/roba:docs
   ```
   This collapses `crates/roba/src/lib.rs` → `src/lib.rs`,
   `docs/roba/profiles.md` → `docs/profiles.md`, drops every
   commit that didn't touch those paths.
4. **Rename roba → roba everywhere**:
   - `Cargo.toml`: `name = "roba"`, `[[bin]] name = "roba"`,
     description tweak
   - `src/main.rs`: `use roba::...`, `roba::dispatch(...)`,
     `roba::classify_exit_code(...)`
   - `README.md`, `CHANGELOG.md`, all `docs/*.md` files: every
     `roba` reference
   - `src/cli.rs`: help text mentions, `[command(name = "roba")]`
     if we set one explicitly
   - `src/profile.rs`: `~/.config/roba/` → `~/.config/roba/`,
     `.roba/profiles.toml` → `.roba/profiles.toml`,
     `ROBA_PROFILE` → `ROBA_PROFILE`,
     `ROBA_PROFILES_FILE` → `ROBA_PROFILES_FILE`,
     `--no-default-profile` (unchanged), starter file mentions
   - `src/starter_profiles.toml`: any `roba` in comments
   - `tests/cli.rs`, `tests/live.rs`: bin name references
   - Drop "roba design notes" subtitle in `docs/design-notes.md`
5. **Switch claude-wrapper to crates.io**:
   ```toml
   # Cargo.toml
   [dependencies]
   claude-wrapper = "0.9"   # or whatever current version is
   ```
   Resolve any breakage if 0.9's API has drifted.
6. **Wire up CI** -- copy `.github/workflows/` shape from
   claude-wrapper, adjust crate name. release-plz config too.
7. **Add LICENSE-APACHE and LICENSE-MIT** at repo root (copy
   from claude-wrapper).
8. **Commit + push.** `git remote set-url origin <new repo>`,
   `git push -u origin main`.
9. **Tag v0.1.0** -- doesn't need to publish yet; just tag the
   first stable.

### Config migration (one-time, document for users)

The config dir renames force a one-time mv for existing users (you,
mainly):

```bash
mv ~/.config/roba ~/.config/roba
# in any project with project-local profiles:
mv .roba .roba
```

Could ship a `roba profile migrate` helper that does this in one
go, but probably overkill for 0.1.

## Post-move cleanup (in claude-wrapper repo)

- [ ] Remove `crates/roba/` (directory)
- [ ] Remove `docs/roba/` (directory)
- [ ] Update workspace `Cargo.toml` -- drop `crates/roba` from
      members
- [ ] Update `release-plz.toml` -- drop the roba `[[package]]`
      block
- [ ] Update root `README.md` -- remove the roba Crates entry,
      note "roba spun off as `roba` at https://github.com/.../roba"
- [ ] Update root `CHANGELOG.md` -- add an entry "roba spun off
      to its own repo as `roba`"
- [ ] `cargo build --workspace && cargo test --workspace` --
      ensure claude-wrapper builds cleanly without roba
- [ ] Commit as `chore: spin off roba to its own repo as roba`
- [ ] PR + merge to main

## Verification

- [ ] `cargo install --path .` in the new repo installs `roba`
- [ ] `which roba` returns nothing (or warns); `which roba` finds
      it
- [ ] Run the field-test snippets (roba "..." → roba "...",
      roba -e → roba -e, profile active, etc.)
- [ ] `cargo publish --dry-run -p roba` clean

## First publish (optional, separate session)

- [ ] Bump version, write a real CHANGELOG entry
- [ ] Verify Cargo.toml has good metadata: description, keywords,
      categories
- [ ] `cargo publish -p roba`
- [ ] Tag + GitHub release with the CHANGELOG entry as the body
