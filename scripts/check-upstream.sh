#!/bin/sh
# Reports commits on hicder/agy-acp's main that this fork has not taken.
#
# This is a hard fork: there is no `upstream` remote and no intention of adding
# one back, so the comparison goes through the GitHub API rather than a remote.
# What "taken" means is whatever sha is written in .upstream-watermark -- a
# deliberate record you move when you have reviewed upstream work, not a thing
# that advances on its own. Upstream rewrote its streaming layer once already;
# a watermark that moved automatically would quietly claim that was reviewed.
#
#     scripts/check-upstream.sh            # report
#     scripts/check-upstream.sh --update   # report, then move the watermark
#
# Exit status: 0 when there is nothing new, 1 when there is (so CI can act on
# it), 2 on error.

set -e

UPSTREAM_REPO=${UPSTREAM_REPO:-hicder/agy-acp}
UPSTREAM_BRANCH=${UPSTREAM_BRANCH:-main}
root=$(git rev-parse --show-toplevel)
watermark_file="$root/.upstream-watermark"

if ! command -v gh >/dev/null 2>&1; then
    echo "check-upstream: needs the gh CLI." >&2
    exit 2
fi

if [ ! -f "$watermark_file" ]; then
    echo "check-upstream: no $watermark_file. Write the last upstream sha you reviewed into it." >&2
    exit 2
fi

watermark=$(tr -d '[:space:]' <"$watermark_file")
head=$(gh api "repos/$UPSTREAM_REPO/branches/$UPSTREAM_BRANCH" --jq '.commit.sha')

if [ "$watermark" = "$head" ]; then
    echo "Up to date with $UPSTREAM_REPO@$UPSTREAM_BRANCH ($(echo "$head" | cut -c1-7))."
    exit 0
fi

count=$(gh api "repos/$UPSTREAM_REPO/compare/$watermark...$head" --jq '.ahead_by')
echo "$count new commit(s) on $UPSTREAM_REPO@$UPSTREAM_BRANCH since $(echo "$watermark" | cut -c1-7):"
echo
gh api "repos/$UPSTREAM_REPO/compare/$watermark...$head" \
    --jq '.commits | reverse | .[] | "  " + .sha[0:7] + "  " + (.commit.message | split("\n")[0])'
echo
echo "Files touched:"
gh api "repos/$UPSTREAM_REPO/compare/$watermark...$head" \
    --jq '.files[] | "  " + (.changes|tostring) + "\t" + .filename' | sort -rn | head -20
echo
echo "To read the code without adding a remote:"
echo "    git fetch https://github.com/$UPSTREAM_REPO $UPSTREAM_BRANCH:refs/heads/hicder-snapshot"

if [ "$1" = "--update" ]; then
    echo "$head" >"$watermark_file"
    echo
    echo "Watermark moved to $(echo "$head" | cut -c1-7). Commit it."
fi

exit 1
