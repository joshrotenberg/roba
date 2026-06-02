#!/usr/bin/env bash
#
# Aggregate the repo's skills/, agents/, and docs/ into a flat mdbook
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
mkdir -p "$SRC/skills" "$SRC/agents" "$SRC/docs"

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

# 3. Skills: each skills/<name>/SKILL.md -> book/src/skills/<name>.md
for d in skills/*/; do
  name="$(basename "$d")"
  [ -f "$d/SKILL.md" ] || continue
  render_page "$d/SKILL.md" "skills/$name/SKILL.md" "$SRC/skills/$name.md"
done

# 4. Agents: each agents/<name>/AGENT.md -> book/src/agents/<name>.md
for d in agents/*/; do
  name="$(basename "$d")"
  [ -f "$d/AGENT.md" ] || continue
  render_page "$d/AGENT.md" "agents/$name/AGENT.md" "$SRC/agents/$name.md"
done

# 5. Top-level docs (exclude docs/README.md, the index).
for f in docs/*.md; do
  base="$(basename "$f")"
  [ "$base" = "README.md" ] && continue
  render_page "$f" "$f" "$SRC/docs/$base"
done

# 5b. The examples subtree is referenced from SUMMARY + the README;
# copy it verbatim (README + the .yml it links to) so links resolve.
if [ -d docs/examples ]; then
  mkdir -p "$SRC/docs/examples/github-actions"
  render_page "docs/examples/github-actions/README.md" \
    "docs/examples/github-actions/README.md" \
    "$SRC/docs/examples/github-actions/README.md"
  cp docs/examples/github-actions/pr-review.yml \
    "$SRC/docs/examples/github-actions/pr-review.yml"
fi

# 6. Section intros (no frontmatter; render_page handles that).
render_page "skills/README.md" "skills/README.md" "$SRC/skills/README.md"
render_page "agents/README.md" "agents/README.md" "$SRC/agents/README.md"

# 7. Rewrite cross-reference links for the flattened tree.
#
# The source files use directory-shaped relative links
# (../<name>/SKILL.md, ../../agents/<name>/AGENT.md, etc.). In the
# flattened book each skill/agent is a single file, so:
#   - a link to a SKILL.md becomes <name>.md   (from a skill page)
#                              or ../skills/<name>.md (from an agent page)
#   - a link to an AGENT.md becomes ../agents/<name>.md (from a skill)
#                              or <name>.md   (from an agent page)
# We apply skill-page rules to everything under skills/ and agent-page
# rules to everything under agents/, which is correct regardless of the
# original prefix because the rules key off the link *target* type.

# Skill pages: SKILL.md targets -> sibling; AGENT.md targets -> ../agents/
for f in "$SRC"/skills/*.md; do
  sed -i.bak -E \
    -e 's#\]\(((\.\./)*(skills/)?)([a-z0-9_-]+)/SKILL\.md([)#])#](\4.md\5#g' \
    -e 's#\]\(((\.\./)*(agents/)?)([a-z0-9_-]+)/AGENT\.md([)#])#](../agents/\4.md\5#g' \
    "$f"
  rm -f "$f.bak"
done

# Agent pages: SKILL.md targets -> ../skills/; AGENT.md targets -> sibling
for f in "$SRC"/agents/*.md; do
  sed -i.bak -E \
    -e 's#\]\(((\.\./)*(skills/)?)([a-z0-9_-]+)/SKILL\.md([)#])#](../skills/\4.md\5#g' \
    -e 's#\]\(((\.\./)*(agents/)?)([a-z0-9_-]+)/AGENT\.md([)#])#](\4.md\5#g' \
    "$f"
  rm -f "$f.bak"
done

# Bare-directory pointers in the section intros -> their overview pages.
sed -i.bak -E 's#\]\(\.\./agents/\)#](../agents/README.md)#g' "$SRC/skills/README.md"
rm -f "$SRC/skills/README.md.bak"
sed -i.bak -E 's#\]\(\.\./skills/\)#](../skills/README.md)#g' "$SRC/agents/README.md"
rm -f "$SRC/agents/README.md.bak"

# 8. Generate SUMMARY.md from the filesystem walk (alphabetical).
SUMMARY="$SRC/SUMMARY.md"
{
  printf '# Summary\n\n'
  printf '[Introduction](README.md)\n\n'

  printf '## Skills\n\n'
  printf -- '- [Overview](skills/README.md)\n'
  for f in $(ls "$SRC"/skills/*.md | sort); do
    base="$(basename "$f" .md)"
    [ "$base" = "README" ] && continue
    printf -- '- [%s](skills/%s.md)\n' "$base" "$base"
  done
  printf '\n'

  printf '## Agents\n\n'
  printf -- '- [Overview](agents/README.md)\n'
  for f in $(ls "$SRC"/agents/*.md | sort); do
    base="$(basename "$f" .md)"
    [ "$base" = "README" ] && continue
    printf -- '- [%s](agents/%s.md)\n' "$base" "$base"
  done
  printf '\n'

  printf '## Docs\n\n'
  for f in $(ls "$SRC"/docs/*.md | sort); do
    base="$(basename "$f" .md)"
    printf -- '- [%s](docs/%s.md)\n' "$base" "$base"
  done
  if [ -f "$SRC/docs/examples/github-actions/README.md" ]; then
    printf -- '- [GitHub Actions example](docs/examples/github-actions/README.md)\n'
  fi
} > "$SUMMARY"

echo "Aggregated book source into $SRC"
