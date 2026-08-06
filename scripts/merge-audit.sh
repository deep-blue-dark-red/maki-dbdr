#!/usr/bin/env bash
# Post-merge silent-drift auditor.
#
# `git merge` only flags overlapping-line conflicts. It stays silent about two
# other failure modes that are just as common and much harder to spot by eye:
#
#   1. DUPLICATE INSERT — both sides add the same (or near-same) top-level
#      item — a fn, const, struct, Cargo.toml key — in a region that doesn't
#      textually overlap, so git keeps both. Compiles to E0428/E0592/"duplicate
#      key", or for Cargo.toml, a manifest load failure.
#   2. STALE DRIFT — a file that isn't conflicted at all because only one side
#      touched it, but the other side's *unconflicted* neighboring files moved
#      on (renamed a method, changed a signature) and this file now calls the
#      old shape. Compiles clean or, worse, compiles wrong (wrong string
#      constant, wrong field) and only shows up as a test failure.
#
# Run this after `git merge` reports "fix conflicts and then commit", once
# every UU marker is gone but BEFORE you commit. It has nothing to do with
# resolving the marked conflicts — it's for the files git merged with no
# complaint at all.
#
# Usage:
#   scripts/merge-audit.sh                  # mid-merge: diffs ORIG_HEAD vs MERGE_HEAD
#   scripts/merge-audit.sh <ours> <theirs>  # explicit refs (e.g. after the fact)
#
# Exit 0 if nothing needs attention, 1 if drift or duplicate symbols were found.

set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

if [ "$#" -ge 2 ]; then
    OURS="$1"
    THEIRS="$2"
elif [ -f .git/MERGE_HEAD ] && [ -f .git/ORIG_HEAD ]; then
    OURS="$(cat .git/ORIG_HEAD)"
    THEIRS="$(cat .git/MERGE_HEAD)"
else
    echo "No merge in progress and no <ours> <theirs> given." >&2
    echo "Usage: $0 [<ours-ref> <theirs-ref>]" >&2
    exit 2
fi

echo "Auditing merge: ours=$(git rev-parse --short "$OURS") theirs=$(git rev-parse --short "$THEIRS")"
echo ""

# ── Step 1: still-conflicted files ──────────────────────────────────────────
# Not this script's job to resolve, but if any remain the rest of the audit
# is meaningless (working tree still has literal <<<<<<< markers in it).
CONFLICTED=$(git diff --name-only --diff-filter=U 2>/dev/null || true)
if [ -n "$CONFLICTED" ]; then
    echo "‼ Unresolved conflicts still present — resolve these first, then re-run:"
    echo "$CONFLICTED" | sed 's/^/    /'
    echo ""
fi

# ── Step 2: silent-drift scan ────────────────────────────────────────────────
# Every file that differs between the two parents is a candidate. If the
# working copy matches one parent exactly, that side's resolution "won" and
# there's nothing to check. If it matches neither, the merge synthesized
# something new for that file — which is sometimes correct (a real 3-way
# merge, or one side's independent unrelated fix folded in for free) and
# sometimes the duplicate-insert or stale-drift bug above. Either way it
# needs a human to look, so we just report it.
mapfile -t CANDIDATES < <(git diff --name-only "$OURS" "$THEIRS" -- 2>/dev/null | while read -r f; do
    git cat-file -e "HEAD:$f" 2>/dev/null && echo "$f"
done)

DRIFT_FILES=()
for f in "${CANDIDATES[@]}"; do
    [ -f "$f" ] || continue
    if git show "$OURS:$f" 2>/dev/null | diff -q - "$f" >/dev/null 2>&1; then
        continue
    fi
    if git show "$THEIRS:$f" 2>/dev/null | diff -q - "$f" >/dev/null 2>&1; then
        continue
    fi
    DRIFT_FILES+=("$f")
done

# ── Step 3: duplicate-symbol heuristic ───────────────────────────────────────
# Cheap, no build required: pull every MODULE-LEVEL (column-0, i.e. outside
# any impl/fn/mod body) fn/const/struct/enum, and every Cargo.toml `key =`
# line scoped to its enclosing [section], then flag names repeated within
# that scope. This is exactly the E0428/E0592/"duplicate key" pattern.
#
# Deliberately column-0-only for Rust: trait methods legitimately repeat by
# name across different `impl X for Y` blocks in the same file (`fn drop`,
# `fn stream_message`, ...) — those are indented and not a collision. Only a
# name declared directly in the module scope collides.
check_rust_dupes() {
    local f="$1"
    # name:line pairs, so each occurrence can be checked for a preceding cfg.
    local hits
    hits=$(grep -nE '^(pub(\([a-z]+\))?[[:space:]]+)?(async[[:space:]]+)?(fn|const|struct|enum)[[:space:]]+[A-Za-z_][A-Za-z0-9_]*' "$f" 2>/dev/null \
        | sed -E 's/^([0-9]+):(pub(\([a-z]+\))?[[:space:]]+)?(async[[:space:]]+)?(fn|const|struct|enum)[[:space:]]+([A-Za-z_][A-Za-z0-9_]*).*/\6 \1/')
    [ -z "$hits" ] && return
    local dup_names
    dup_names=$(echo "$hits" | awk '{print $1}' | sort | uniq -d)
    [ -z "$dup_names" ] && return
    while read -r name; do
        [ -z "$name" ] && continue
        local lines cfg_gated=0
        lines=$(echo "$hits" | awk -v n="$name" '$1==n{print $2}')
        for ln in $lines; do
            # #[cfg(...)] is almost always the line directly above the item
            # (possibly with a doc/derive line between); check a small window.
            if sed -n "$((ln > 4 ? ln - 4 : 1)),$((ln - 1))p" "$f" | grep -q '#\[cfg('; then
                cfg_gated=1
            fi
        done
        [ "$cfg_gated" -eq 1 ] && continue
        echo "$name"
    done <<< "$dup_names"
}

# Scoped per [table]/[[table]] header, since the same key legitimately
# repeats across sections (`name` in [package] and [[bin]], `tempfile` in
# [dependencies] and [dev-dependencies]).
check_toml_dupes() {
    local f="$1"
    awk '
        /^\[/ { section = $0; delete seen; next }
        match($0, /^[A-Za-z0-9_-]+[[:space:]]*=/) {
            key = $0
            sub(/[[:space:]]*=.*/, "", key)
            if (key in seen) print section "  " key
            seen[key] = 1
        }
    ' "$f" 2>/dev/null
}

DUP_HITS=()
for f in "${CANDIDATES[@]}"; do
    [ -f "$f" ] || continue
    case "$f" in
        *.rs)
            dupes=$(check_rust_dupes "$f")
            if [ -n "$dupes" ]; then
                DUP_HITS+=("$f")
                echo "⚠ duplicate symbol(s) in $f:"
                echo "$dupes" | sed 's/^/    /'
            fi
            ;;
        Cargo.toml|*/Cargo.toml)
            dupes=$(check_toml_dupes "$f")
            if [ -n "$dupes" ]; then
                DUP_HITS+=("$f")
                echo "⚠ duplicate key(s) in $f:"
                echo "$dupes" | sed 's/^/    /'
            fi
            ;;
    esac
done
# dedupe DUP_HITS (a file can appear via both loops)
if [ "${#DUP_HITS[@]}" -gt 0 ]; then
    mapfile -t DUP_HITS < <(printf '%s\n' "${DUP_HITS[@]}" | sort -u)
fi

echo ""
echo "── Files that merged clean on neither side's exact content ────────────────"
if [ "${#DRIFT_FILES[@]}" -eq 0 ]; then
    echo "  (none — every changed file matches one parent exactly)"
else
    for f in "${DRIFT_FILES[@]}"; do
        marker=""
        for d in "${DUP_HITS[@]:-}"; do
            [ "$d" = "$f" ] && marker=" [duplicate symbols — see above]"
        done
        echo "  $f$marker"
    done
fi

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
printf "║  merge-audit: %2d drift file(s), %2d file(s) with dup symbols  ║\n" "${#DRIFT_FILES[@]}" "${#DUP_HITS[@]}"
echo "╚══════════════════════════════════════════════════════════════╝"

if [ -n "$CONFLICTED" ]; then
    echo ""
    echo "Resolve the unresolved conflicts above before trusting this report."
    exit 1
fi

if [ "${#DUP_HITS[@]}" -gt 0 ]; then
    echo ""
    echo "Fix the duplicate symbols first — they are near-certain build breaks."
fi

if [ "${#DRIFT_FILES[@]}" -gt 0 ]; then
    echo ""
    echo "For each drift file above, decide by hand:"
    echo "  - matches neither side because it's a genuine 3-way merge → fine, leave it"
    echo "  - matches neither side because one side's unrelated fix rode along → fine, leave it"
    echo "  - matches neither side because of duplicate/stale content → fix it"
    echo "One quick check per file:"
    echo "  diff <(git show $OURS:<file>) <file>   # what ours would have looked like"
    echo "  diff <(git show $THEIRS:<file>) <file> # what theirs would have looked like"
fi

if [ "${#DRIFT_FILES[@]}" -eq 0 ] && [ "${#DUP_HITS[@]}" -eq 0 ]; then
    exit 0
else
    exit 1
fi
