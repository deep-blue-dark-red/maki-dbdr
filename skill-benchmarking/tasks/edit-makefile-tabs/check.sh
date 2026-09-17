#!/usr/bin/env bash
set -euo pipefail
cd "$WORKDIR"
grep -q "$(printf '\t')rsync -az \$(DIST)/ server-b:/srv/app" Makefile
grep -q "$(printf '\t')ssh server-b 'systemctl restart demoapp'" Makefile
! grep -q 'server-a' Makefile
make -n deploy > /dev/null
