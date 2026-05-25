# Migration plan: cwr → roba, claude-wrapper workspace → own repo

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
      roba needs. Spot check by building `cwr` against the *crates.io*
      version of claude-wrapper instead of the path dep -- temporarily
      swap in `Cargo.toml`, `cargo build -p cwr`, confirm clean, then
      revert.
- [ ] Run the full `cwr` test suite one more time (unit +
      mechanical CLI). Live tests too if you want to bank a green
      run.
- [ ] Note current cwr commit SHA for reference in the new repo's
      first commit message ("forked from claude-wrapper@SHA").

## The move

### Pick a history strategy

| Strategy | Pro | Con |
|---|---|---|
| **Fresh start** | Clean log starting at `Initial roba.` Faster to set up. | Loses ~45 commits of design history and rationale. |
| **`git filter-repo --subdirectory-filter`** | Preserves the per-commit history with full messages. The "we built this incrementally" trail stays intact. | Tooling adds friction (filter-repo isn't in core git); some commits touched both claude-wrapper and cwr, those need cleanup. |
| **Hybrid: squash + curated CHANGELOG** | Single "Initial roba" commit, but the CHANGELOG.md has the milestones. Tells the story without preserving line-by-line. | Manual write-up of the CHANGELOG entries. |

Lean: **filter-repo**. Worth the half-hour setup -- the commit
messages from this session are themselves design docs.

### Steps (assuming filter-repo)

1. **Create new GitHub repo** `joshrotenberg/roba`. Don't init
   with README / LICENSE / .gitignore; we're pushing existing
   content.
2. **Clone claude-wrapper to a working dir** specifically for
   surgery: `git clone --no-local <claude-wrapper> roba-extract`.
3. **Filter to roba-relevant paths** (`crates/cwr/`,
   `docs/cwr/`, root files we want):
   ```bash
   cd roba-extract
   git filter-repo \
     --path crates/cwr \
     --path docs/cwr \
     --path-rename crates/cwr:. \
     --path-rename docs/cwr:docs
   ```
   This collapses `crates/cwr/src/lib.rs` → `src/lib.rs`,
   `docs/cwr/profiles.md` → `docs/profiles.md`, drops every
   commit that didn't touch those paths.
4. **Rename cwr → roba everywhere**:
   - `Cargo.toml`: `name = "roba"`, `[[bin]] name = "roba"`,
     description tweak
   - `src/main.rs`: `use roba::...`, `roba::dispatch(...)`,
     `roba::classify_exit_code(...)`
   - `README.md`, `CHANGELOG.md`, all `docs/*.md` files: every
     `cwr` reference
   - `src/cli.rs`: help text mentions, `[command(name = "roba")]`
     if we set one explicitly
   - `src/profile.rs`: `~/.config/cwr/` → `~/.config/roba/`,
     `.cwr/profiles.toml` → `.roba/profiles.toml`,
     `CWR_PROFILE` → `ROBA_PROFILE`,
     `CWR_PROFILES_FILE` → `ROBA_PROFILES_FILE`,
     `--no-default-profile` (unchanged), starter file mentions
   - `src/starter_profiles.toml`: any `cwr` in comments
   - `tests/cli.rs`, `tests/live.rs`: bin name references
   - Drop "cwr design notes" subtitle in `docs/design-notes.md`
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
mv ~/.config/cwr ~/.config/roba
# in any project with project-local profiles:
mv .cwr .roba
```

Could ship a `roba profile migrate` helper that does this in one
go, but probably overkill for 0.1.

## Post-move cleanup (in claude-wrapper repo)

- [ ] Remove `crates/cwr/` (directory)
- [ ] Remove `docs/cwr/` (directory)
- [ ] Update workspace `Cargo.toml` -- drop `crates/cwr` from
      members
- [ ] Update `release-plz.toml` -- drop the cwr `[[package]]`
      block
- [ ] Update root `README.md` -- remove the cwr Crates entry,
      note "cwr spun off as `roba` at https://github.com/.../roba"
- [ ] Update root `CHANGELOG.md` -- add an entry "cwr spun off
      to its own repo as `roba`"
- [ ] `cargo build --workspace && cargo test --workspace` --
      ensure claude-wrapper builds cleanly without cwr
- [ ] Commit as `chore: spin off cwr to its own repo as roba`
- [ ] PR + merge to main

## Verification

- [ ] `cargo install --path .` in the new repo installs `roba`
- [ ] `which cwr` returns nothing (or warns); `which roba` finds
      it
- [ ] Run the field-test snippets (cwr "..." → roba "...",
      cwr -e → roba -e, profile active, etc.)
- [ ] `cargo publish --dry-run -p roba` clean

## First publish (optional, separate session)

- [ ] Bump version, write a real CHANGELOG entry
- [ ] Verify Cargo.toml has good metadata: description, keywords,
      categories
- [ ] `cargo publish -p roba`
- [ ] Tag + GitHub release with the CHANGELOG entry as the body
