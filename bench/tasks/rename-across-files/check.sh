#!/usr/bin/env bash
set -euo pipefail
cd "$WORKDIR"
! grep -rnw 'load_cfg' --include='*.py' .
grep -q 'load_config' core/settings.py
grep -q 'load_config' tests/test_settings.py
python3 -m unittest discover -q
