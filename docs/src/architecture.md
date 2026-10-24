# Architecture

This chapter is the contract the code is written against. If code and this
document disagree, one of them has a bug.

## The problem in one paragraph

Every parallel agent needs its own git worktree. A fresh worktree builds from
cold (minutes on a real workspace). Pointing every worktree at one shared
`CARGO_TARGET_DIR` serializes builds on Cargo's lock and, worse, produces
silently wrong builds: Cargo's freshness check compares source mtimes against
its fingerprint files, so agent B's older edit looks "fresh" next to agent A's
newer artifact and B ships A's code with zero crates rebuilt. Ferry forks one
immutable warm build per agent instead, and makes the fork look fresh to Cargo
only where that is provably true.

## Invariants

Each one is enforced in code (module named) and covered by tests.

1. **Never stamp a file whose content differs from the base** (`freshness`).
   A tracked file's mtime is set to the fixed stamp time only if it is a
   regular file at merge stage 0, its mode and blob id equal the base
   commit's tree entry, its working-tree bytes equal the base checkout's
   bytes, and its metadata did not change between verification and stamping.
   A directory is stamped only if its entry names equal the base checkout's
   directory, because Cargo's `rerun-if-changed=<dir>` takes the recursive max
   mtime of the directory, its subdirectories and its files, and a deletion
   is only visible as a directory mtime change. `StampPlan` can only be built
   by the verifying function, so nothing else can stamp. Stamping happens once,
   at spawn; a live agent tree is never re-stamped.
2. **Bases are immutable once built** (`base`). A base is keyed by commit plus
   a fingerprint of toolchain, warm commands and modes. `base.json` is written
   last, atomically, and doubles as the "ready" marker; nothing writes to a
   ready base. Refreshing builds a new base; `gc` removes a base only when it
   is not current and no agent references it.
3. **Each agent owns its build state** (`fork`, `sandbox`, `agent`). Forks are
   reflink clones (new inodes), overlayfs upper layers (base is a read-only
   lower), or full copies. There is no hardlink backend: Cargo rewrites
   fingerprint files with `File::create` (truncating the existing inode), so a
   hardlinked fork would let an agent's rebuild modify the base. Cargo always
   gets `CARGO_TARGET_DIR` *and* `CARGO_BUILD_BUILD_DIR` pointing at the fork,
   so a user's `build.build-dir` (whose `{workspace-path-hash}` is identical for
   every agent under the canonical mount) cannot redirect intermediates into a
   shared directory.
4. **The agent is derived from the working directory, never from inherited
   environment** (`agent`, `shim`). Agent tools copy their session environment
   into sub-agents that work in other worktrees; an inherited
   `CARGO_TARGET_DIR` or agent name would point a sub-agent at someone else's
   build state. Ferry never exports `CARGO_TARGET_DIR` to anything but the
   cargo process it starts.
5. **Refuse unsafe states with a clear error** (everywhere): no base, base
   checkout or target missing (stamping cannot be verified), agent not fully
   created, overlay agent without a namespace, overlay upper dir still mounted
   by an unjoinable namespace, store paths overlayfs cannot express, dirty
   worktree on `rm`.

## Module layout

```text
src/
  main.rs        multicall: `cargo` (shim) or `ferry` (cli)
  lib.rs         module list; compile_error! on non-unix
  cli/           clap definitions, command handlers, human/JSON output (anyhow)
  config.rs      ferry.toml schema, platform overrides, FERRY_* env, Settings
  context.rs     Repo + Settings + Store for one invocation
  git.rs         shells out to git (worktrees, ls-tree, ls-files, status)
  store.rs       on-disk layout, ids, repo key
  lock.rs        flock(2) wrapper
  fsutil.rs      atomic JSON writes, disk usage, forced removal
  toolchain.rs   rustc/cargo version detection per worktree
  freshness.rs   stamping plan + application, freshness mode, relocation fixups
  base.rs        build / list / gc of immutable bases
  agent.rs       spawn / rm / list / env, session and cargo exec, reaping
  shim.rs        the cargo shim (PATH entry → per-call agent resolution)
  hook.rs        agent-tool hooks (Claude Code WorktreeCreate/Remove, SessionStart)
  fork/          Backend trait + selection; reflink (Linux FICLONE), clonefile
                 (macOS), overlay (Linux), copy; shared tree walker
  sandbox/       Plan, enter(), users(); linux.rs: user+mount namespaces,
                 canonical bind mounts, overlay mount, join-or-create
  platform/      Capabilities probes (fs type, reflink, userns, overlay, sysctls)
  doctor.rs      checks + fixes + "what spawn would do"
```

Git is driven through the `git` binary rather than libgit2 or gix: worktree
creation must honour the user's hooks, filters, LFS, `core.autocrlf` and sparse
checkout exactly as `git` does, because any divergence changes file contents
relative to what the base was built from.

## On-disk layout

```text
<store parent>/                    $FERRY_HOME, [store] dir, or the platform cache dir
  bin/cargo -> /path/to/ferry      the shim
  <repo-name>-<hash>/              one store per repository (hash of git common dir)
    store.json  lock  current
    mnt/src  mnt/target            canonical mountpoints (empty on the host)
    bases/<commit>-<fp>/           immutable once base.json exists
      base.json  src/  target/
    bases/<commit>-<fp>.lock
    agents/<name>/
      agent.json  lock
      src/                         default worktree location (--path overrides)
      target/                      reflink / copy backends
      upper/  work/  ns.json       overlay backend
```

Everything writable lives under one directory, so an agent tool's sandbox
needs a single writable-root entry for Ferry.

## Lifecycle

### `ferry base [--rev R]`

1. Resolve `R`, detect the toolchain, resolve freshness mode and isolation
   (probe user namespaces in a child process when `sandbox = auto`).
2. Compute the base id; take `bases/<id>.lock`. If `base.json` exists, make it
   current and stop. Otherwise discard any partial directory.
3. `git worktree add --detach` the checkout, `git worktree lock` it, stamp all
   tracked files and directories (the base is the reference, nothing to
   compare).
4. Enter the sandbox (irreversible for this process; host paths stay
   reachable), run each warm command with `sh -c`, stdout redirected to stderr.
5. Record files the build wrote into the source tree (build scripts writing
   outside `OUT_DIR`) as warnings; write `base.json`; set `current`.

### `ferry spawn NAME [--from REF] [--path DIR] [--branch B]`

1. Under the store lock: check name, branch and path are free; choose the base
   (current if it is at `REF`, else any ready base at `REF`, else current);
   decide isolation; write `agent.json` with `state: creating`; take the agent
   lock.
2. `git worktree add -b <prefix><name>`, plan stamping against the base
   (invariant 1), stamp, `git update-index --refresh`.
3. Select the backend, fork the base target, and when the agent runs without
   the namespace sandbox, invalidate units whose dep-info embeds base paths
   (see below). Write `state: ready`. Any failure rolls back worktree, branch
   and agent dir.

### `ferry run [NAME] -- CMD` and the shim

`ferry run` (and `ferry shell`) enter the agent's sandbox and exec `CMD` with
the working directory left at its host path, `PATH` prefixed with the shim
directory, and `FERRY_AGENT=<name>` set for scripts. Host paths keep working
inside the sandbox; agent tools that key state on their cwd (transcripts,
memory) keep one entry per agent.

The shim (`bin/cargo -> ferry`) is where builds are redirected. On every call
it resolves the agent from the working directory: a host path inside an agent
worktree, or a path under `mnt/src` whose mounted inode is an agent worktree.
For an agent it enters (or confirms it is already in) that agent's sandbox,
moves to the canonical path, sets `CARGO_TARGET_DIR` and
`CARGO_BUILD_BUILD_DIR` and the freshness env, and execs the real cargo (the
next `cargo` on `PATH` that is not ferry). Outside agent worktrees it execs the
real cargo unchanged.

### `ferry rm NAME [--force]`

Refuses if processes run in the agent's sandbox or the worktree is dirty
(unless `--force`). Removes the worktree and fork; deletes the branch only if
it still points at the agent's starting commit, otherwise keeps it and reports
how many commits it carries.

### `ferry gc [--dry-run]`

Reaps abandoned hook-created agents (clean, no new commits, idle, not
`git worktree lock`ed by their tool), then deletes bases that are neither
current nor referenced, skipping any base whose lock is held.

## Fork backends

```rust
pub trait Backend {
    fn kind(&self) -> BackendKind;
    fn fork(&self, paths: &ForkPaths) -> Result<ForkStats, ForkError>;
    fn target_spec(&self, paths: &ForkPaths) -> TargetSpec;
    fn private_bytes(&self, paths: &ForkPaths) -> Result<u64, ForkError>;
}
```

| Backend | Platforms | Fork cost | Notes |
|---|---|---|---|
| `reflink` | Linux (btrfs, XFS, bcachefs, OpenZFS 2.2+), macOS (APFS) | per-file clone (Linux), one `clonefile(2)` (macOS) | target is a real dir; hard links inside the tree preserved; mtimes preserved |
| `overlay` | Linux 5.11+, namespace sandbox required | O(1) | copy-up copies whole files on first write; mount lives only inside the sandbox |
| `copy` | everywhere | full copy | fallback |

`auto` picks reflink when the store filesystem supports it, else overlay when
the agent runs in a namespace sandbox and overlayfs works there, else copy. An
explicit backend that is unavailable is an error, never a silent fallback.

## Sandbox (Linux)

`sandbox::enter(plan)` runs in the calling process, which must be
single-threaded (`unshare(CLONE_NEWUSER)` fails with `EINVAL` otherwise):

1. `unshare(CLONE_NEWUSER | CLONE_NEWNS)`, write `setgroups=deny`, map the
   caller's uid and gid to themselves (not root: some agent tools refuse to run
   as root, and the util-linux `mount` CLI misbehaves under that mapping, so
   Ferry calls `mount(2)` directly).
2. Make `/` recursively private.
3. Bind-mount the worktree onto `mnt/src`; bind-mount the target dir, or mount
   overlayfs (`lowerdir=base target, upperdir, workdir, userxattr`), onto
   `mnt/target`.

Two overlay mounts must never use the same upper dir at once. For overlay
agents, `enter` therefore runs under the agent lock and either *joins* a live
member of the agent's recorded mount namespace (`setns` into its user and mount
namespaces) or creates a new one and records its identity in `ns.json`. If the
only remaining users are in nested namespaces that cannot be joined, it
refuses. Bind-mount-only sandboxes (reflink, copy) are safe to create per call.

Without namespaces (macOS; Linux with `sandbox = off` or where user
namespaces are blocked) builds run at host paths. Crate-level reuse still
works; the first edit pays for rustc's incremental state being keyed on
absolute paths. Because units that captured absolute paths of the base
(`env!("CARGO_MANIFEST_DIR")`, `OUT_DIR`, workspace-relative `include_str!` on
Cargo <= 1.100) would otherwise stay "fresh" while pointing at the base, spawn
deletes the fork's fingerprints for exactly those units, so Cargo rebuilds them.

## Freshness modes

| Mode | When | What Cargo compares |
|---|---|---|
| `mtime` | default on stable | source mtime vs fingerprint (strictly newer = dirty) |
| `checksum` | `auto` on nightly toolchains, or explicit | rustc dep-info inputs by size + blake3; build-script inputs, outputs-vs-deps and rustdoc stay mtime-based |

`checksum` needs `CARGO_UNSTABLE_CHECKSUM_FRESHNESS=true` and (nightly
2026-09-02 and later) `CARGO_BUILD_FINGERPRINT=content`; Ferry sets both for the
base and every agent. Switching mode rebuilds every unit, so the mode is a
property of the base. Stamping runs in both modes because build scripts stay
mtime-based.

## Concurrency

Locks are `flock(2)` on files in the store and are never held while a build
runs:

| Lock | Held by | Duration |
|---|---|---|
| `<store>/lock` | spawn (registry step), rm, gc, setting `current` | milliseconds to seconds |
| `bases/<id>.lock` | `ferry base` for that id; gc skips held ones | the base build |
| `agents/<name>/lock` | spawn (whole), rm, sandbox entry for runs | spawn; ~1 ms per run |

Lock order is store, then agent, then base.

## Errors and output

Library code returns typed errors (`thiserror`); the CLI wraps them with
`anyhow` and prints `error:`, `caused by:` and a `hint:` from
`Error::hint()`. Data goes to stdout, progress to stderr, colour only on a
terminal. Every read command and every mutating command accepts `--json`.
