#!/bin/sh
# Smoke test for scripts/run-timeout.pl (used by .githooks/pre-push).
# Run from repo root: sh tests/pre-push-timeout.sh

set -e

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
run_timeout() {
	secs=$1
	shift
	perl "$ROOT/scripts/run-timeout.pl" "$secs" "$@"
}

run_timeout 5 true

start=$(date +%s)
run_timeout 10 true
elapsed=$(( $(date +%s) - start ))
if [ "$elapsed" -gt 2 ]; then
	echo "run_timeout hung for ${elapsed}s after fast command (expected <2s)" >&2
	exit 1
fi

set +e
run_timeout 5 sh -c 'exit 42'
status=$?
set -e
if [ "$status" -ne 42 ]; then
	echo "expected exit 42 from failing command, got $status" >&2
	exit 1
fi

set +e
start=$(date +%s)
run_timeout 2 sleep 60
status=$?
elapsed=$(( $(date +%s) - start ))
set -e
if [ "$status" -ne 124 ]; then
	echo "expected exit 124 on timeout, got $status" >&2
	exit 1
fi
if [ "$elapsed" -gt 4 ]; then
	echo "timeout path took ${elapsed}s (expected <=4s)" >&2
	exit 1
fi

set +e
start=$(date +%s)
run_timeout 3 sh -c 'trap "" TERM; sleep 60'
status=$?
elapsed=$(( $(date +%s) - start ))
set -e
if [ "$elapsed" -gt 6 ]; then
	echo "SIGTERM-immune child hung run_timeout for ${elapsed}s" >&2
	exit 1
fi
if [ "$status" -ne 124 ]; then
	echo "expected exit 124 for SIGTERM-immune timeout, got $status" >&2
	exit 1
fi

echo "pre-push-timeout smoke ok"
