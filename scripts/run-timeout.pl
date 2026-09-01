#!/usr/bin/env perl
# Portable timeout for .githooks/pre-push (GNU timeout is not on macOS).
# Usage: run-timeout.pl SECS COMMAND [ARGS...]
# Exit 124 when the alarm fires; otherwise the child's exit code.
use strict;
use warnings;
use POSIX qw(setpgid);

my $secs = shift @ARGV;
die "usage: $0 SECS COMMAND [ARGS...]\n" unless defined $secs && @ARGV;

my $pid = fork();
die "fork: $!\n" unless defined $pid;

if ($pid == 0) {
	setpgid(0, 0) or die "setpgid: $!\n";
	exec @ARGV or die "exec: $!\n";
}

my $pgid = $pid;
$SIG{ALRM} = sub {
	kill -15, $pgid;
	select(undef, undef, undef, 0.5);
	kill -9, $pgid;
};
alarm($secs + 1);
waitpid($pid, 0);
alarm(0);

my $status = $?;
if ($status == 0) { exit 0 }
my $sig = $status & 127;
if ($sig == 15 || $sig == 9) { exit 124 }
exit($sig ? $sig : ($status >> 8));
