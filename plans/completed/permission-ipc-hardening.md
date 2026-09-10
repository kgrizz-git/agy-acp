# Harden the permission bridge's private runtime resources

## Outcome

Make the Unix permission bridge own an unguessable, private per-process runtime
directory for its hook definition and socket, and fail closed under malformed,
slow, or excessive local connections. Cleanup must remove only resources this
instance created.

## Evidence and scope

Today `PermissionBridge::start` binds a predictable
`$TMPDIR/agy-acp-perm-<pid>.sock`, removes any file at that path first, and
spawns an unbounded task for every accepted connection. `HookRoot::create`
similarly creates `$TMPDIR/agy-acp-hooks-<pid>` with `create_dir_all`, sweeps
all prefix-matching old directories, and later recursively removes them. Both
read a whole line or stdin payload without a size limit. The evidence and its
limits are recorded in
[`dev-docs/research/investigation-notes.md`](../dev-docs/research/investigation-notes.md#security-and-permission-boundaries).

This work protects the adapter's own files and prevents accidental or
cross-user interference through the system temporary directory. It does not
sandbox a hostile process running as the same OS user: a workspace lifecycle
hook already executes with that user's authority outside the bridge. Nor does
it change the ACP permission policy, remembered-answer keying, or agy's hook
semantics.

## Design invariants

- Each enabled adapter owns a newly created, random directory below the system
  temporary directory with Unix mode `0700`; its socket and hook root have no
  predictable public pathname. Create with a Unix `mkdir` mode of `0700`, then
  verify/restore that mode only on the directory just created: `umask` can make
  it stricter but cannot make it more permissive, so this never creates a
  public window or needs a process-wide umask change.
- Creation is exclusive and boundedly retried on a name collision. A collision,
  malformed pre-existing entry, or setup failure is an error; it is never
  unlinked, reused, chmodded, or recursively removed by this adapter.
- The hook root passed to `agy --add-dir` is its own child directory inside the
  owner, distinct from the socket's parent. It may remain read-only without
  making socket cleanup impossible; cleanup first restores permissions on that
  owned child and its `.agents` directory.
- The bridge accepts only one semantically valid, at-most-1-MiB JSON frame per
  connection, and at most eight live connections. The permit spans reading,
  host permission wait, and response write, so pending host requests are also
  bounded. The same compile-time limit applies to hook stdin and bridge
  responses; they run from the same binary, so no cross-version negotiation is
  needed.
- Frames that cannot parse, exceed the limit, time out, or lack a non-empty
  `toolCall.name` deny without asking the ACP host. Capacity exhaustion also
  returns a bounded explicit deny when its peer can accept one; no rejection
  path can become an allow.
- Normal shutdown and handled `SIGTERM`, `SIGINT`, and `SIGHUP` clean up the
  owned runtime directory through one idempotent explicit cleanup operation,
  not `Arc` reference counts or task termination. `SIGKILL` and machine loss
  cannot run cleanup; their random, owner-private remnants are inert and are
  deliberately not swept by pathname prefix on a later startup.
- The socket pathname stays below the Unix-domain socket path-length limit on
  supported macOS and Linux systems. A path builder uses a short fixed socket
  name and token, checks against the conservative 104-byte macOS limit before
  bind, and fails closed if the selected temporary directory is too long.

## Work

1. Introduce one private-runtime owner that creates the short random `0700`
   directory using an exclusive `mkdir`-style primitive, bounded retries on
   `AlreadyExists`, and no action on an entry it did not create. Build a short,
   testable socket path beneath it, reject an overlong path before bind, and
   give `HookRoot` its own child directory. Retain an explicit, idempotent
   cleanup handle for the adapter lifetime; do not make removal depend on an
   accept-task clone becoming the last `Arc` owner.
2. Remove PID-derived naming, unconditional socket unlinking, and broad stale
   root sweeping. Make partial setup unwind only the owner object from step 1.
   Its cleanup restores the mode of its read-only hook-root child and `.agents`
   directory before removal, reports an error rather than swallowing one, and
   never recursively removes a path merely because it has a familiar prefix.
3. Put the 1-MiB frame limit in a shared module used by both bridge and hook
   subcommand. Bound hook stdin, socket request, and socket response reads;
   apply read and write deadlines; parse and validate `toolCall.name` before
   calling `decide`. In the accept loop, acquire one of eight permits before
   spawning and keep it for the full read → prompt → write lifetime. A saturated
   listener sends a bounded explicit denial directly without allocating another
   long-lived task.
4. Give `install_shutdown_killer` a shared `Mutex<Option<RuntimeCleanup>>`
   handle before permission prompts are initialized, then populate it after the
   runtime owner is created. On a handled signal, kill live children, invoke
   that idempotent cleanup, report any cleanup failure, then call
   `process::exit`. Call the same cleanup explicitly on ordinary event-loop
   exit; retain `Drop` only as a no-panic fallback. Do not sweep remnants from
   unhandleable exits.
5. Add focused Unix tests: secure mode, distinct paths, and conservative
   path-length rejection through an injected-base path builder; collision leaves
   a sentinel unchanged; ordinary, signal-path, and partial-start cleanup remove
   only the owned directory and succeed when the hook child is read-only; valid
   hook round trips still work; malformed, semantically empty, oversized, slow,
   and oversized-response frames deny without a host request; and all eight
   permits remain occupied through pending host answers so a ninth connection
   is denied without growing pending work. Update fixtures affected by the
   new runtime/HookRoot construction API.
6. Update the README, `AGENTS.md` runtime-path/cleanup description, relevant
   investigation-note status, and Rust docstrings for the private-runtime
   cleanup contract, frame-limit rationale, permit lifetime, and intentional
   `SIGKILL` remnant behavior. Bump `Cargo.toml` to `0.2.0` and move all current
   `## Unreleased` entries under that heading with the security change in the
   same pull request.

## Acceptance

- No normal runtime pathname is derived solely from the PID, and no startup or
  cleanup path deletes, changes permissions on, or reuses an entry it did not
  create.
- The socket's parent directory is private, the hook directory remains
  read-only to agy and is restored before its owned cleanup, and both work on
  supported Unix targets within socket path limits.
- A valid hook request retains the existing host prompt and explicit-decision
  behavior; malformed, semantically empty, oversized, slow, or surplus peers
  cannot create a host prompt, consume unbounded memory/tasks, or obtain an
  allow.
- Handled shutdown removes the owned resources; an unhandled shutdown leaves no
  later startup cleanup action capable of deleting a prefix-matching attacker
  path.
- `cargo fmt`, the unit tier, and Clippy pass.

## Rollback

The protocol remains an environment-provided socket path and a one-line JSON
request/response, so reverting the implementation restores the prior release
without persisted-state migration. If a path-length or platform compatibility
problem is found before release, disable permission prompts fail-closed rather
than falling back to predictable shared-temporary paths.
