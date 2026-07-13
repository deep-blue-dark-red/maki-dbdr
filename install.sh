#!/bin/sh
set -eu

REPO="tontinton/maki"
BINARY="maki"
VIEWER="mlog"
INSTALL_DIR="${MAKI_INSTALL_DIR:-/usr/local/bin}"

main() {
    need_cmd curl

    case "$(uname -s)" in
        Linux)  os="unknown-linux-musl" ;;
        Darwin) os="apple-darwin" ;;
        *) err "unsupported OS: $(uname -s)" ;;
    esac

    case "$(uname -m)" in
        x86_64|amd64)   arch="x86_64" ;;
        aarch64|arm64)  arch="aarch64" ;;
        *) err "unsupported architecture: $(uname -m)" ;;
    esac

    target="${arch}-${os}"

    tag="${1:-$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' | cut -d'"' -f4)}"
    [ -n "${tag}" ] || err "failed to determine latest release tag"

    url="https://github.com/${REPO}/releases/download/${tag}/${BINARY}-${tag}-${target}.tar.gz"
    tmp="$(mktemp -d)"
    trap 'rm -rf "${tmp}"' EXIT

    echo "downloading ${BINARY} ${tag} for ${target}..."
    curl -fsSL "${url}" | tar xz -C "${tmp}"

    # The .mlog log viewer ships alongside maki; install it too if present.
    bins="${BINARY}"
    [ -f "${tmp}/${VIEWER}" ] && bins="${bins} ${VIEWER}"

    if [ -w "${INSTALL_DIR}" ]; then
        for b in ${bins}; do mv "${tmp}/${b}" "${INSTALL_DIR}/${b}"; done
    else
        echo "installing to ${INSTALL_DIR} (requires sudo)..."
        for b in ${bins}; do sudo mv "${tmp}/${b}" "${INSTALL_DIR}/${b}"; done
    fi

    for b in ${bins}; do chmod +x "${INSTALL_DIR}/${b}"; done
    echo "${bins} ${tag} installed to ${INSTALL_DIR}"
    echo ""
}

need_cmd() {
    command -v "$1" > /dev/null 2>&1 || err "need '$1' (not found)"
}

err() {
    echo "error: $1" >&2
    exit 1
}

main "$@"
