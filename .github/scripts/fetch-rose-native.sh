#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# fetch-rose-native.sh — pull the rose engine library for one release target out of
# THIS release and stage it at ./lib, where the Package steps pick it up.
#
# WHY IT IS AN ASSET AND NOT A CHECKOUT. maki ships the engine library so a user gets
# one archive and one version. The library is built by the dotmodel2vec repo, whose
# origin is a private Forgejo — GitHub runners cannot clone it — so the artifacts are
# built on hosts of each target OS and uploaded to this release beforehand:
#
#   rose-native-<tag>-<target>.tar.gz | .zip     containing  lib/<library>
#
# built by dotmodel2vec's scripts/pack-rose-native.sh, which prints the exact
# `gh release upload` line to run.
#
# ABSENT IS NOT AN ERROR. This runs in the release legs before packaging; an archive
# without the asset still produces a working maki (the library is optional by design),
# so a missing asset is a warning that names the file it looked for, not a failure.
#
# usage: fetch-rose-native.sh <tag> <target>        (target = the Rust triple)
# ---------------------------------------------------------------------------
set -euo pipefail

TAG="${1:?usage: fetch-rose-native.sh <tag> <target>}"
TARGET="${2:?usage: fetch-rose-native.sh <tag> <target>}"

REPO="${REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

case "$TARGET" in
    *windows*) ext=zip ;;
    *)         ext=tar.gz ;;
esac
ASSET="rose-native-${TAG}-${TARGET}.${ext}"

if ! gh release download "$TAG" -p "$ASSET" -D "$WORK" --clobber 2>/dev/null; then
    echo "::warning::no $ASSET on $TAG; packaging maki without the rose engine library"
    exit 0
fi

mkdir -p "$REPO/lib"
if [ "$ext" = zip ]; then
    unzip -qo "$WORK/$ASSET" -d "$REPO/lib"
else
    tar xzf "$WORK/$ASSET" -C "$REPO/lib"
fi

# The archive carries lib/<library>; flatten the extra level so the staged tree is
# exactly ./lib/<library>.
if [ -d "$REPO/lib/lib" ]; then
    mv "$REPO/lib/lib"/* "$REPO/lib/" 2>/dev/null || true
    rmdir "$REPO/lib/lib" 2>/dev/null || true
fi

echo "staged $(ls "$REPO/lib" | tr '\n' ' ')"
