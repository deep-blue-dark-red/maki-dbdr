#!/usr/bin/env bash
# Build maki in release mode and install it to /usr/local/bin.
set -euo pipefail

usage() {
    cat <<EOF
Usage: $(basename "$0")

Build maki with 'cargo build -r' and copy the release binary to
/usr/local/bin/maki with sudo.
EOF
}

case "${1:-}" in
    -h | --help)
        usage
        exit 0
        ;;
esac

cargo build -r
sudo cp /Users/mcp/.cache/cargo-target/release/maki /usr/local/bin/maki
