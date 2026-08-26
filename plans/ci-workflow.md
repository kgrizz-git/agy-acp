# Plan: Add build, deterministic test, and secret-gated e2e workflows

> **TODO discipline:** Keep the corresponding `TODO.md` entry and its Next Up
> pointer until this work lands. They are linked to this plan; delete them only
> in the landing change.

## Decisions

- The required CI gate is `cargo build`, unit tests, and ignored I/O tests.
  E2e is deliberately separate because it uses a paid external service.
- CI must land after `fix-test-read-response-from-db.md`; the ignored tier is
  knowingly red until that stale helper is removed.
- E2e runs for pull requests to `main` only after approval of a protected `e2e`
  GitHub environment. Store its `E2E_GEMINI_API_KEY` as an environment secret,
  not a repository secret, because the job checks out PR-controlled code. It is
  also manually dispatchable.
- Pin agy to `1.1.16`, the current release selected for this workflow. The
  Ubuntu x64 archive and SHA-256 are recorded below so a release change cannot
  silently alter the test environment. The integrity value was checked against
  the public release metadata on 2026-08-26. The version still needs the
  compatibility preflight below because the bridge behaviours documented in
  `AGENTS.md` were observed with agy 1.1.12.
- The repository promises Rust 1.70+. Declare that MSRV in `Cargo.toml` and
  test it in CI; `rust-version` alone declares a contract but does not verify
  it. The adapter must build and pass unit tests on Windows too. Its optional
  Unix-socket permission bridge is unavailable there and must fail closed rather
  than disabling agy's own permission checks.

## `ci.yml`: always-on build and deterministic tests

Create `.github/workflows/ci.yml` with:

- Triggers: `push` to `main`, `pull_request` targeting `main`, and
  `workflow_dispatch`.
- Minimal permissions: `contents: read`.
- Concurrency:
  ```yaml
  concurrency:
    group: ${{ github.workflow }}-${{ github.ref }}
    cancel-in-progress: true
  ```
- A stable `test` job using SHA-pinned `actions/checkout` with
  `persist-credentials: false`, SHA-pinned `dtolnay/rust-toolchain` with
  `toolchain: stable` and `profile: minimal`, and SHA-pinned
  `Swatinem/rust-cache`.
- A 15-minute job timeout.
- The following steps, in order:
  ```sh
  cargo build --verbose
  cargo test --verbose
  cargo test --verbose -- --ignored --skip e2e
  ```

The last command runs only ignored tests and excludes the four `test_e2e_*`
tests by their stable shared substring. Add a short workflow comment explaining
the e2e exclusion and update it if that naming convention changes.

Add an `msrv` matrix job for Ubuntu and Windows using the SHA-pinned
`dtolnay/rust-toolchain` action with `toolchain: 1.70`, the same secure
checkout/cache setup, and a 15-minute timeout. It must run
`cargo build --verbose` and `cargo test --verbose`. Add `rust-version = "1.70"`
to the package metadata in
`Cargo.toml`. If current dependencies cannot meet that toolchain, either pin
compatible dependency versions or explicitly raise the documented MSRV; do not
leave the README's existing `1.70+` promise unverified.

Do not add `cargo fmt --check` or a new clippy gate: the repository is not kept
rustfmt-clean. This is a policy decision, not an accidental omission.

## `e2e.yml`: secret-gated external integration test

Create `.github/workflows/e2e.yml` with `pull_request` targeting `main` and
`workflow_dispatch` triggers, top-level `permissions: contents: read`, and this
two-job shape:

1. `gate` runs in the protected `e2e` environment and writes `has_key` to
   `$GITHUB_OUTPUT` after checking whether `E2E_GEMINI_API_KEY` is nonempty. Set
   up that environment with required reviewers, add the secret there, and remove
   any repository-level e2e key. Do this in a step environment; secrets cannot
   be referenced directly in an `if:` expression. Use this exact shape:
   ```yaml
   gate:
     runs-on: ubuntu-latest
     environment: e2e
     outputs:
       has_key: ${{ steps.key.outputs.has_key }}
     steps:
       - id: key
         env:
          GEMINI_API_KEY: ${{ secrets.E2E_GEMINI_API_KEY }}
         run: |
           if [ -n "$GEMINI_API_KEY" ]; then
             echo 'has_key=true' >>"$GITHUB_OUTPUT"
           else
             echo 'has_key=false' >>"$GITHUB_OUTPUT"
           fi
   ```
2. `e2e` needs `gate` and uses
   `if: needs.gate.outputs.has_key == 'true'`. It is visibly skipped when the
   secret is absent. Set `environment: e2e`, `timeout-minutes: 20`, and
   `permissions: contents: read`. Do not replace this with `pull_request_target`:
   checking out untrusted PR code with the API key would expose the secret.

The `e2e` job uses the same checkout, Rust setup, and cache as `ci.yml`, then
performs these steps:

1. Use one `Install agy` shell step for the download, digest check, extraction,
   installation, and version check. The shell variables below must stay in that
   one `run` block; they do not persist between GitHub Actions steps.
   ```sh
   AGY_VERSION=1.1.16
   AGY_ARCHIVE="$RUNNER_TEMP/agy_cli_linux_x64.tar.gz"
   AGY_SHA256=7742953b7835b457e9102f1357a493913657dfd147435584f609d58356ec085a
   curl --fail --location --retry 3 \
     --output "$AGY_ARCHIVE" \
     "https://github.com/google-antigravity/antigravity-cli/releases/download/${AGY_VERSION}/agy_cli_linux_x64.tar.gz"
   echo "${AGY_SHA256}  ${AGY_ARCHIVE}" | sha256sum --check --strict
   AGY_EXTRACT="$RUNNER_TEMP/agy-extract"
   mkdir -p "$AGY_EXTRACT" "$RUNNER_TEMP/bin"
   tar -xzf "$AGY_ARCHIVE" -C "$AGY_EXTRACT"
   AGY_BIN=$(find "$AGY_EXTRACT" -type f -name agy -perm -u+x -print -quit)
   test -n "$AGY_BIN"
   install -m 755 "$AGY_BIN" "$RUNNER_TEMP/bin/agy"
   echo "$RUNNER_TEMP/bin" >>"$GITHUB_PATH"
   "$RUNNER_TEMP/bin/agy" --version
   ```
   Extraction or the version check must fail the job; do not rely on the Rust
   tests' intentional local-development self-skip when `agy` is unavailable.
2. Configure a fresh runner for key-based agy authentication before starting the
   adapter:
   ```sh
   mkdir -p "$HOME/.gemini/antigravity-cli"
   printf '%s\n' '{"modelProvider":"gemini"}' \
     >"$HOME/.gemini/antigravity-cli/settings.json"
   ```
   Pass `GEMINI_API_KEY` only through the environment of the e2e test step.
   Do not set `AGY_EXTRA_ARGS`; the workflow needs no hidden provider flags.
3. Run:
   ```sh
   cargo build --release --verbose
   cargo test e2e -- --ignored --nocapture
   ```

The pinned release is newer than the first agy release that documented
`GEMINI_API_KEY`; its key mode requires `modelProvider: "gemini"`. Keep a
workflow comment beside the pin stating that both the asset digest and this
configuration are deliberate compatibility inputs.

## Required compatibility preflight

Before merging `e2e.yml`, install or otherwise select agy `1.1.16` locally,
confirm `agy --version`, set `GEMINI_API_KEY` and `modelProvider: "gemini"`,
then run:

```sh
cargo build --release
cargo test e2e -- --ignored --nocapture
```

Record the agy version and result in the PR. This proves the release pin works
with the adapter's normal streaming protocol before CI becomes the first
compatibility experiment. The four e2e tests do not start the adapter with
`--permission-prompts`, so this preflight does **not** replace the separate
real-client permission-bridge verification tracked in `TODO.md`.

## Documentation and landing

- Add one concise `AGENTS.md` note: CI enforces the build/unit/ignored-I/O tiers;
  Rust 1.70 is tested on Linux and Windows; the Unix-socket permission bridge
  is unavailable on Windows; the e2e workflow requires approval of a protected
  environment and uses a pinned agy release.
- Add a `CHANGELOG.md` Maintenance entry naming `ci.yml` and `e2e.yml`, the
  ignored-tier e2e exclusion, and the absence of a formatting gate.
- Keep both TODO pointers through implementation and review. Delete the CI
  subsection and its Next Up pointer only in the landing change. Do not touch
  the DB-test item; its companion plan owns that removal.

## Verification

1. Confirm GitHub Actions is enabled for `kgrizz-git/agy-acp` before declaring
   the work complete.
2. On a PR without an approved `e2e` environment containing
   `E2E_GEMINI_API_KEY`, `test` succeeds and the `e2e` job is
   skipped—not failed or rejected during workflow parsing.
3. With an approved environment secret, verify the e2e job reports the pinned agy version,
   verifies the SHA-256, and runs all four e2e tests. A missing binary or failed
   download must fail the job rather than produce a green self-skip.
4. Verify the stable `test` job and both Rust 1.70 `msrv` matrix jobs pass. The
   MSRV jobs are the proof behind the README's Rust 1.70+ requirement.
