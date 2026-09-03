#!/bin/sh
# Fails when a Rust source file outgrows reading.
#
# The cap is deliberately loose: it is not a style rule, it is a tripwire for the
# thing that actually happened here once already -- permission.rs reaching 3724
# lines because two thirds of it was an inline test module nobody was counting.
#
# Files already over the cap are listed below with their current size as a frozen
# ceiling, so they cannot grow while the cap is being worked towards. Lower a
# ceiling when a file shrinks; never raise one. An entry removed is an entry that
# came in under CAP, which is the point.
#
# A gate that fails open is worse than no gate, so the parsing here is defensive
# about its own inputs: every way this script was found to pass when it should
# have failed is guarded below, and the guard says which one tripped.
set -eu

CAP=1200

# path<TAB>ceiling
EXEMPT=$(cat <<'ENTRIES'
src/adapter.rs	1262
ENTRIES
)

# Validate the list before trusting it per-file. A duplicated path made awk print
# two ceilings, `[ "$lines" -gt "$ceiling" ]` then failed as a non-integer
# comparison, and because the test sits in an `if` condition `set -e` did not
# fire -- the error read as "not over the ceiling" and a 1301-line file passed.
# A space instead of a tab silently yielded an empty ceiling with the same
# effect. Both are self-inflicted, and both disarm the check quietly, which is
# the failure mode this whole script exists to prevent.
printf '%s\n' "$EXEMPT" | awk -F'\t' '
    $0 == "" { next }
    NF != 2 || $1 == "" || $2 !~ /^[0-9]+$/ {
        printf "BAD EXEMPT entry, want path<TAB>ceiling: %s\n", $0 >"/dev/stderr"
        bad = 1
    }
    { seen[$1]++ }
    END {
        for (path in seen) {
            if (seen[path] > 1) {
                printf "BAD EXEMPT: %s listed %d times\n", path, seen[path] >"/dev/stderr"
                bad = 1
            }
        }
        exit bad ? 1 : 0
    }
'

files=$(git ls-files '*.rs')
if [ -z "$files" ]; then
    echo "FAIL: no tracked Rust files found. Run this from the repository root." >&2
    exit 1
fi

status=0

# Read line by line rather than `for file in $(git ls-files)`: word splitting
# breaks a path containing a space into two, and neither half exists, so the real
# file is never measured. Fed by a here-document, not a pipe, so the loop runs in
# this shell and `status` survives it.
while IFS= read -r file; do
    [ -n "$file" ] || continue
    # awk counts records, so a final line with no trailing newline still counts.
    # `wc -l` counts newlines and would report 1200 for a 1201-line file.
    lines=$(awk 'END { print NR }' "$file")
    ceiling=$(printf '%s\n' "$EXEMPT" | awk -F'\t' -v f="$file" '$1 == f { print $2 }')

    if [ -n "$ceiling" ]; then
        if [ "$lines" -gt "$ceiling" ]; then
            echo "FAIL $file: $lines lines, above its frozen ceiling of $ceiling."
            echo "     This file is already over the $CAP-line cap. It may shrink, not grow."
            status=1
        elif [ "$lines" -le "$CAP" ]; then
            echo "STALE $file: $lines lines, now under the $CAP-line cap."
            echo "      Drop its entry from EXEMPT in $0."
            status=1
        fi
    elif [ "$lines" -gt "$CAP" ]; then
        echo "FAIL $file: $lines lines, over the $CAP-line cap."
        echo "     Split it, or add it to EXEMPT in $0 with a reason it cannot be."
        status=1
    fi
done <<FILES
$files
FILES

# An entry whose file was deleted or renamed is never visited by the loop above,
# so it would sit in EXEMPT forever, reserving a ceiling for nothing and quietly
# exempting the path if it ever came back.
while IFS= read -r path; do
    [ -n "$path" ] || continue
    if ! printf '%s\n' "$files" | grep -Fxq -- "$path"; then
        echo "STALE $path: listed in EXEMPT but not a tracked Rust file."
        echo "      Drop its entry from EXEMPT in $0."
        status=1
    fi
done <<PATHS
$(printf '%s\n' "$EXEMPT" | awk -F'\t' '{ print $1 }')
PATHS

[ "$status" -eq 0 ] && echo "All Rust files within the $CAP-line cap."
exit "$status"
