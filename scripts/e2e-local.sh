#!/usr/bin/env bash
# Local e2e runner: reproduces the CI e2e tier against the token in
# .env.e2e.local, without touching the user's real ~/.gemini state.
#
# Usage:
#   scripts/e2e-local.sh [filter] [extra cargo-test args...]
#
#   scripts/e2e-local.sh                         # full tier (3 model turns!)
#   scripts/e2e-local.sh test_e2e_error_paths    # plumbing only, no model calls
#   scripts/e2e-local.sh test_e2e_session_load --nocapture
#
# The first argument is a test-name filter only if it does not start with `-`;
# anything else is passed to the test binary (no `--` needed; the script
# supplies it). The defaults mirror CI: `-- --ignored --nocapture --test-threads=1`.
#
# What it walls off: everything runs under a throwaway HOME (mktemp dir, or
# $E2E_HOME to reuse one), so agy and agy-acp read and write
# $HOME/.gemini/... (settings.json, conversation DB, installation UUID)
# inside the sandbox. Your OAuth login, real settings.json, and session
# history are never read or written through file paths.
#
# Caveat: the macOS keychain is global, not under HOME. If agy finds your
# OAuth token there it may authenticate as you rather than as the token —
# watch the `[e2e] model:` lines and, on failure, the `[agy-acp stderr]`
# lines to see which identity a run actually used.
#
# Env overrides:
#   E2E_HOME           reuse a sandbox dir (keeps the conversation DB across runs)
#   E2E_MODEL_ROSTER   comma-separated slugs for rotation tests (default: empty
#                      → tests use the settings.json default, as in CI today)
#   E2E_MODEL_OFFSET   rotation offset (default: 0)
#   E2E_MODEL_LABEL    settings.json fallback label (default: Gemini 3.6 Flash (Low))
set -euo pipefail

cd "$(dirname "$0")/.."

ENV_FILE=".env.e2e.local"
if [ ! -f "$ENV_FILE" ]; then
    echo "error: $ENV_FILE not found; create it with: export GEMINI_API_KEY=<key>" >&2
    exit 1
fi
# shellcheck disable=SC1091
set -a; source "$ENV_FILE"; set +a
if [ -z "${GEMINI_API_KEY:-}" ]; then
    echo "error: GEMINI_API_KEY is empty after sourcing $ENV_FILE" >&2
    exit 1
fi

if ! command -v agy >/dev/null 2>&1; then
    echo "error: agy not found in PATH" >&2
    exit 1
fi

BIN="target/release/agy-acp"
if [ ! -x "$BIN" ]; then
    echo "release binary missing; building (required for e2e tests)..."
    cargo build --release
fi

if [ -n "${E2E_HOME:-}" ]; then
    SANDBOX="$E2E_HOME"
    mkdir -p "$SANDBOX"
else
    SANDBOX=$(mktemp -d "${TMPDIR:-/tmp}/agy-acp-e2e-home.XXXXXX")
fi
# Point HOME at the sandbox AFTER pinning the toolchain locations: rustup
# resolves its toolchains via $HOME when RUSTUP_HOME/CARGO_HOME are unset,
# so overriding HOME alone would hide the installed toolchains.
REAL_HOME="$HOME"
export RUSTUP_HOME="${RUSTUP_HOME:-$REAL_HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}"
export HOME="$SANDBOX"
mkdir -p "$HOME/.gemini/antigravity-cli"
SETTINGS="$HOME/.gemini/antigravity-cli/settings.json"

# Mirror CI: settings.json first (agy models needs it to list), then the
# roster query. python3 builds the JSON so labels with quotes stay valid.
E2E_MODEL_LABEL="${E2E_MODEL_LABEL:-Gemini 3.6 Flash (Low)}"
SETTINGS="$SETTINGS" E2E_MODEL_LABEL="$E2E_MODEL_LABEL" python3 -c \
    'import json, os; json.dump({"modelProvider": "gemini", "model": os.environ["E2E_MODEL_LABEL"]}, open(os.environ["SETTINGS"], "w"))'
test -s "$SETTINGS"

export E2E_MODEL_ROSTER="${E2E_MODEL_ROSTER:-}"
export E2E_MODEL_OFFSET="${E2E_MODEL_OFFSET:-0}"

echo "sandbox HOME:  $SANDBOX"
echo "settings.json: $(cat "$SETTINGS")"
echo "roster:        ${E2E_MODEL_ROSTER:-<empty>} (offset $E2E_MODEL_OFFSET)"

FILTER="e2e"
if [ "$#" -gt 0 ] && [ "${1#-}" = "$1" ]; then
    FILTER="$1"
    shift
fi
exec cargo test "$FILTER" -- --ignored --nocapture --test-threads=1 "$@"
