#!/usr/bin/env bash
set -euo pipefail
cd "$WORKDIR"
grep -q '__version__ = "1.2.0"' pkg/version.py
head -1 CHANGELOG.md | grep -q '# Changelog'
grep -q '## 1.2.0' CHANGELOG.md
grep -Eq '^\s*[-*] ' CHANGELOG.md
grep -q 'version-1.2.0' README.md
! grep -q 'version-1.1.0' README.md
