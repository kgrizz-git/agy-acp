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
set -eu

CAP=1200

# path<TAB>ceiling
EXEMPT=$(cat <<'ENTRIES'
src/adapter.rs	1262
ENTRIES
)

status=0
for file in $(git ls-files '*.rs'); do
    lines=$(wc -l < "$file" | tr -d ' ')
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
done

[ "$status" -eq 0 ] && echo "All Rust files within the $CAP-line cap."
exit "$status"
