#!/usr/bin/env bash
#
# Aggregate the repo's README, docs/, and skills/ into a flat mdbook
# source tree under book/src/, then generate SUMMARY.md.
#
# Idempotent: nukes and recreates book/src/ on every run. The output
# tree (book/src/ and book/book/) is generated and gitignored; only
# book.toml and this script are committed.
#
# Run from anywhere; paths are resolved relative to the repo root.

set -euo pipefail

# Resolve repo root (the parent of this script's directory).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SRC="$SCRIPT_DIR/src"

GH_BASE="https://github.com/joshrotenberg/roba/blob/main"

cd "$REPO_ROOT"

# 1. Clean + recreate the generated source tree.
rm -rf "$SRC"
mkdir -p "$SRC/docs"

# render_page <srcfile> <gh-relative-path> <destfile>
#
# Copies a markdown file, converting any leading YAML frontmatter
# (--- ... ---) into a fenced ```yaml block so it stays visible
# without mdbook trying to parse it, and appends a Source footer
# pointing at the file on GitHub. Files without frontmatter (the
# section-intro READMEs) pass through unchanged apart from the footer.
render_page() {
  local srcfile="$1" ghpath="$2" destfile="$3"
  awk '
    BEGIN { infm = 0; first = 1 }
    first == 1 && $0 == "---" { infm = 1; first = 0; print "```yaml"; next }
    { first = 0 }
    infm == 1 && $0 == "---" { infm = 0; print "```"; print ""; next }
    infm == 1 { print; next }
    { print }
  ' "$srcfile" > "$destfile"
  {
    printf '\n---\n\n'
    printf '*Source: [`%s`](%s/%s)*\n' "$ghpath" "$GH_BASE" "$ghpath"
  } >> "$destfile"
}

# 2. Introduction page (repo README).
render_page "README.md" "README.md" "$SRC/README.md"
# The repo README points at the top-level dirs; turn the bare-dir
# pointer into the example's page so it resolves in the book.
sed -i.bak -E 's#\]\(docs/examples/github-actions/\)#](docs/examples/github-actions/README.md)#g' "$SRC/README.md"
rm -f "$SRC/README.md.bak"

# 3. Top-level docs (exclude docs/README.md, the index).
for f in docs/*.md; do
  base="$(basename "$f")"
  [ "$base" = "README.md" ] && continue
  render_page "$f" "$f" "$SRC/docs/$base"
done

# 3b. The examples subtree is referenced from SUMMARY + the README;
# copy it verbatim (README + the .yml it links to) so links resolve.
if [ -d docs/examples ]; then
  mkdir -p "$SRC/docs/examples/github-actions"
  render_page "docs/examples/github-actions/README.md" \
    "docs/examples/github-actions/README.md" \
    "$SRC/docs/examples/github-actions/README.md"
  cp docs/examples/github-actions/pr-review.yml \
    "$SRC/docs/examples/github-actions/pr-review.yml"
fi

# 3c. Skills (skills/*/SKILL.md).
if [ -d skills ]; then
  mkdir -p "$SRC/skills"
  for skill_dir in skills/*/; do
    skill_file="${skill_dir}SKILL.md"
    [ -f "$skill_file" ] || continue
    dirname="$(basename "$skill_dir")"
    render_page "$skill_file" "$skill_file" "$SRC/skills/$dirname.md"
  done
fi

# 4. Generate SUMMARY.md with explicit three-section audience-driven layout.
#
# Section order: Getting started, Using roba, Reference.
# Within each section, page order is hard-coded (not alphabetical) to match
# the audience-driven framing. mdbook treats top-level `# Title` lines as
# part separators, rendering them as section headers in the sidebar.
SUMMARY="$SRC/SUMMARY.md"
{
  printf '# Summary\n\n'
  printf '[Introduction](README.md)\n\n'

  # -------------------------------------------------------------------------
  # Getting started
  # -------------------------------------------------------------------------
  printf '# Getting started\n\n'
  if [ -f "$SRC/docs/quickstart.md" ]; then
    printf -- '- [Quickstart](docs/quickstart.md)\n'
  fi
  printf '\n'

  # -------------------------------------------------------------------------
  # Using roba
  # -------------------------------------------------------------------------
  printf '# Using roba\n\n'
  for page in vs-claude-p use-cases profiles aliases permissions scripting; do
    if [ -f "$SRC/docs/$page.md" ]; then
      # Map page filenames to human-readable titles.
      case "$page" in
        vs-claude-p)  title="Why not just claude -p" ;;
        use-cases)    title="Use cases" ;;
        profiles)     title="Profiles" ;;
        aliases)      title="Aliases" ;;
        permissions)  title="Permissions" ;;
        scripting)    title="Scripting / agent ABI" ;;
        *)            title="$page" ;;
      esac
      printf -- '- [%s](docs/%s.md)\n' "$title" "$page"
    fi
  done
  if [ -f "$SRC/docs/examples/github-actions/README.md" ]; then
    printf -- '- [Examples](docs/examples/github-actions/README.md)\n'
  fi
  printf '\n'

  # -------------------------------------------------------------------------
  # Reference
  # -------------------------------------------------------------------------
  printf '# Reference\n\n'
  if [ -f "$SRC/docs/reference.md" ]; then
    printf -- '- [Reference](docs/reference.md)\n'
  fi
  printf '\n'

  # -------------------------------------------------------------------------
  # Skills
  # -------------------------------------------------------------------------
  if [ -d "$SRC/skills" ] && ls "$SRC/skills/"*.md >/dev/null 2>&1; then
    printf '# Skills\n\n'
    for skill_page in "$SRC/skills/"*.md; do
      [ -f "$skill_page" ] || continue
      # Extract the name from frontmatter (name: value), fall back to basename.
      skill_name=$(awk '/^name:/{print $2; exit}' "$skill_page")
      [ -n "$skill_name" ] || skill_name="$(basename "$skill_page" .md)"
      basename_noext="$(basename "$skill_page" .md)"
      printf -- '- [%s](skills/%s.md)\n' "$skill_name" "$basename_noext"
    done
    printf '\n'
  fi
} > "$SUMMARY"

echo "Aggregated book source into $SRC"

# 5. Post-process: rewrite relative .md links to .html for mdbook.
#    External URLs (containing ://) are skipped because [^):] excludes ':'.
#    SUMMARY.md is EXCLUDED: mdbook's table of contents must reference the
#    .md source files. If SUMMARY links are rewritten to .html, mdbook's
#    create-missing generates empty stub source pages that shadow the real
#    renders, leaving every chapter blank. (Regression from the original
#    blanket rewrite.)
find "$SRC" -name "*.md" ! -name "SUMMARY.md" | while read -r file; do
  sed -i.bak -E 's|\]\(([^):]*)\.md(#[^)]+)?\)|\](\1.html\2)|g' "$file" && rm -f "$file.bak"
done
echo "Rewrote .md links to .html in $SRC (excluding SUMMARY.md)"
