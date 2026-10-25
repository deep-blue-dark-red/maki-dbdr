#!/usr/bin/env bash
# Push the current branch to the GitHub fork and the personal repo, then tag
# HEAD and push the tag to both.
set -euo pipefail

GITHUB_REMOTE=${GITHUB_REMOTE:-fork}
PERSONAL_REMOTE=${PERSONAL_REMOTE:-origin}

usage() {
    cat <<EOF
Usage: $(basename "$0") <tag>

Pushes the current branch to '$GITHUB_REMOTE' (GitHub) and '$PERSONAL_REMOTE'
(personal repo), creates <tag> at HEAD, and pushes the tag to both.

Override remotes with GITHUB_REMOTE and PERSONAL_REMOTE.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

case "${1:-}" in
    -h | --help)
        usage
        exit 0
        ;;
    "")
        usage >&2
        die "missing tag"
        ;;
esac

[ $# -eq 1 ] || die "expected exactly one argument (tag), got $#"
TAG=$1

REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || die "not inside a git repository"
cd "$REPO_ROOT"

BRANCH=$(git symbolic-ref --quiet --short HEAD) || die "HEAD is detached; check out a branch first"

for remote in "$GITHUB_REMOTE" "$PERSONAL_REMOTE"; do
    git remote get-url "$remote" >/dev/null 2>&1 || die "remote '$remote' is not configured"
done

git check-ref-format "refs/tags/$TAG" || die "'$TAG' is not a valid tag name"
if git rev-parse --verify --quiet "refs/tags/$TAG" >/dev/null; then
    die "tag '$TAG' already exists (latest: $(git describe --tags --abbrev=0 2>/dev/null || echo none))"
fi

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    die "working tree has uncommitted changes; commit or stash them first"
fi

for remote in "$GITHUB_REMOTE" "$PERSONAL_REMOTE"; do
    echo "==> pushing branch $BRANCH to $remote"
    git push "$remote" "HEAD:refs/heads/$BRANCH"
done

echo "==> tagging $TAG at $(git rev-parse --short HEAD)"
git tag -- "$TAG"

for remote in "$GITHUB_REMOTE" "$PERSONAL_REMOTE"; do
    echo "==> pushing tag $TAG to $remote"
    git push "$remote" "refs/tags/$TAG"
done

echo "done: branch $BRANCH and tag $TAG pushed to $GITHUB_REMOTE and $PERSONAL_REMOTE"
