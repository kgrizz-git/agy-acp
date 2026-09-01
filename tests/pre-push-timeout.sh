#!/bin/sh
# Smoke test for the portable run_timeout helper in .githooks/pre-push.
# Run from repo root: sh tests/pre-push-timeout.sh

set -e

run_timeout() {
	secs=$1
	shift
	perl -e '
		my $secs = shift @ARGV;
		my $pid = fork();
		die "fork: $!\n" unless defined $pid;
		if ($pid == 0) { exec @ARGV or die "exec: $!\n" }
		$SIG{ALRM} = sub { kill 15, $pid };
		alarm $secs;
		waitpid $pid, 0;
		alarm 0;
		if ($? == 0) { exit 0 }
		if ($? & 127) { exit 124 }
		exit($? >> 8);
	' "$secs" "$@"
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
if [ "$status" -eq 0 ]; then
	echo "expected non-zero exit when command exceeds timeout, got 0" >&2
	exit 1
fi
if [ "$elapsed" -gt 4 ]; then
	echo "timeout path took ${elapsed}s (expected <=4s)" >&2
	exit 1
fi

echo "pre-push-timeout smoke ok"
