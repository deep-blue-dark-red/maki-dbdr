#!/usr/bin/env bash
# Print N random bytes from /dev/urandom in hex, base64, chars, or raw form.
set -euo pipefail

CHARSET='A-Za-z0-9'

usage() {
	echo "usage: ${0##*/} <count> [hex|base64|chars|raw]" >&2
	exit 1
}

[[ $# -ge 1 && $# -le 2 ]] || usage
[[ $1 =~ ^[0-9]+$ ]] || usage

count=$1
format=${2:-hex}

case $format in
hex)
	head -c "$count" /dev/urandom | od -An -tx1 | tr -d ' \n'
	echo
	;;
base64)
	head -c "$count" /dev/urandom | base64
	;;
chars)
	# tr is killed by SIGPIPE once head has enough; that's expected
	set +o pipefail
	LC_ALL=C tr -dc "$CHARSET" < /dev/urandom | head -c "$count"
	echo
	set -o pipefail
	;;
raw)
	head -c "$count" /dev/urandom
	;;
*)
	usage
	;;
esac
