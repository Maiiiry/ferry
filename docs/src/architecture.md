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
   bytes, its metadata did not change between verification and stamping, and
   Cargo cannot reach it through a symlink that resolves differently in the
   agent and the base (both ends count: Cargo records a rustc input read
   through a link by its canonical path, but follows a build script's
   `rerun-if-changed` path through it). A directory is stamped only if its
   entry names equal the base checkout's directory, because Cargo's
   `rerun-if-changed=<dir>` takes the recursive max mtime of the directory,
   its subdirectories and its files, and a deletion is only visible as a
   directory mtime change. `StampPlan` can only be built by the verifying
   function. The one other stamping function, `stamp_base_checkout`, stamps a
   fresh, unseeded base checkout, which is the reference itself; only
   `base::prepare` calls it. Every file and directory that is *not* stamped
   gets an mtime no older than the base (`freshen_skipped`), so a hook that
   restores commit times or a clock that went backwards cannot make it look
   fresh. Stamping happens once, at spawn; a live agent tree is never
   re-stamped.
2. **Bases are immutable once built** (`base`). A base is keyed by commit plus
   a fingerprint of toolchain, warm commands and modes. `base.json` is written
   last, atomically, and doubles as the "ready" marker; nothing writes to a
   ready base. Refreshing builds a new base; `gc` removes a base only when it
   is not current and no agent references it. A refresh is seeded from an
   earlier base only if both have the same *unit inputs*: every manifest,
   lockfile, Cargo config and toolchain file of the commit, the Cargo configs
   above the store, and Cargo's flag variables. A seeded target still holds
   units from the seed's own seeds; with equal inputs the new build selects
   none of them, so every unit it reuses was checked against the content
   stamping compared.
3. **Each agent owns its build state** (`fork`, `sandbox`, `agent`). Forks are
   reflink clones (new inodes), overlayfs upper layers (base is a read-only
   lower), or full copies. There is no hardlink backend: Cargo rewrites
   fingerprint files with `File::create` (truncating the existing inode), so a
   hardlinked fork would let an agent's rebuild modify the base. Cargo always
   gets `CARGO_TARGET_DIR` *and* `CARGO_BUILD_BUILD_DIR` pointing at the fork,
   so a user's `build.build-dir` (whose `{workspace-path-hash}` is identical for
   every agent under the canonical mount) cannot redirect intermediates into a
   shared directory. Each Cargo workspace gets its own target dir: Cargo
   hashes path packages by their path relative to the workspace root, so two
   workspaces sharing one would overwrite each other's units (see the shim).
4. **The agent is derived from the working directory, never from inherited
   environment** (`agent`, `shim`). Agent tools copy their session environment
   into sub-agents that work in other worktrees; an inherited
   `CARGO_TARGET_DIR` or agent name would point a sub-agent at someone else's
   build state. Ferry never exports `CARGO_TARGET_DIR` to anything but the
   cargo process it starts.
5. **Refuse unsafe states with a clear error** (everywhere): no base, base
   checkout or target missing (stamping cannot be verified), agent not fully
   created, overlay agent without a namespace, overlay upper dir still mounted
   by an unjoinable namespace, store paths overlayfs cannot express, an agent
   worktree inside the store or overlapping another agent's, and on `rm` a
   dirty worktree, commits no ref reaches, running processes or an agent
   nested inside.

## Module layout

```text
src/
  main.rs        multicall: `cargo` (shim), `ferry-rustc` (remap), or `ferry` (cli)
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
  remap.rs       experimental path-independent incremental builds without namespaces
  fork/          Backend trait + selection; reflink (Linux FICLONE), clonefile
                 (macOS), overlay (Linux), copy; shared tree walker
  sandbox/       Plan, enter(), users(); linux.rs: user+mount namespaces,
                 canonical bind mounts, overlay mount, join-or-create
  platform/      Capabilities probes (fs type, reflink, userns, overlay, sysctls)
  doctor.rs      checks + fixes + "what spawn would do"; the AppArmor profile text
  setup.rs       `setup apparmor`: facts, a pure decision, the one sudo script
```

Git is driven through the `git` binary rather than libgit2 or gix: worktree
creation must honour the user's hooks, filters, LFS, `core.autocrlf` and sparse
checkout exactly as `git` does, because any divergence changes file contents
relative to what the base was built from.

## On-disk layout

```text
<store parent>/                    $FERRY_HOME, [store] dir, or the platform cache dir
  bin/cargo -> /path/to/ferry      the shim
  bin/ferry-rustc -> ...           experimental remap wrapper
  <repo-name>-<hash>/              one store per repository (hash of git common dir)
    store.json  lock  current
    mnt/src  mnt/target            canonical mountpoints (empty on the host)
    bases/<commit>-<fp>/           immutable once base.json exists
      base.json  src/  target/
    bases/<commit>-<fp>.lock
    agents/<name>/
      agent.json  lock
      busy.lock                    held shared by every process run for the agent
      target/                      reflink / copy backends
        ferry-ws/<path>/           target dirs of workspaces nested at <path>
      upper/  work/                overlay backend
      mount.lock  ns.json          overlay: in-use lock and live namespace identity

<worktree parent>/                 FERRY_WORKTREES, [agent] worktree-dir, $FERRY_HOME/worktrees,
  <repo-name>-<hash>/<name>/       or the platform data dir; --path overrides per agent
```

Build state lives in the cache dir; agent worktrees, which hold uncommitted
work, live in the data dir (`~/.local/share/ferry`, `~/Library/Application
Support/ferry`) because macOS may purge caches and users clear them casually.
All build state Ferry writes is under one directory, so an agent tool's sandbox
needs one writable-root entry for it plus the worktree (its working directory).

## Lifecycle

### `ferry base [--rev R]`

1. Resolve `R`, detect the toolchain, resolve freshness mode and isolation
   (probe user namespaces in a child process when `sandbox = auto`).
2. Compute the base id; take `bases/<id>.lock`. If `base.json` exists, make it
   current and stop. Otherwise discard any partial directory.
3. `git worktree add --detach` the checkout and `git worktree lock` it. If a
   ready base with the same fingerprint and unit inputs exists at another
   commit (and `--cold` is not given), seed from it: clone its target (reflink
   when possible) and stamp only files proven identical to its checkout,
   exactly as for an agent, so the warm build only rebuilds what changed.
   Otherwise stamp all tracked files and directories (the base is the
   reference, nothing to compare).
4. Enter the sandbox (irreversible for this process; host paths stay
   reachable), run each warm command with `sh -c`, stdout redirected to
   stderr, the shim first on `PATH` so each workspace gets the target dir it
   gets in agents.
5. Record files the build wrote into the source tree (build scripts writing
   outside `OUT_DIR`) as warnings; write `base.json`; set `current` before
   releasing the base lock.

### `ferry spawn NAME [--from REF] [--path DIR] [--branch B]`

1. Under the store lock: check name, branch and path are free; choose the base
   (current if it is at `REF`, else any ready base at `REF`, else current);
   decide isolation; write `agent.json` with `state: creating`; take the agent
   lock.
2. `git worktree add -b <prefix><name>`, copy the base's `Cargo.lock` if the
   repository does not track one, plan stamping against the base
   (invariant 1), stamp, raise the mtimes of what was not stamped,
   `git update-index --refresh`.
3. Select the backend, fork the base target, and when the agent runs without
   the namespace sandbox, invalidate its local units (see below). Write
   `state: ready`. A failure rolls back only what this spawn created: the
   worktree if it added it, the branch if it still points at the starting
   commit, and the agent dir.

### `ferry run [NAME] -- CMD` and the shim

`ferry run` (and `ferry shell`) enter the agent's sandbox and exec `CMD` with
the working directory left at its host path, `PATH` prefixed with the shim
directory, and `FERRY_AGENT=<name>` set for scripts. Host paths keep working
inside the sandbox; agent tools that key state on their cwd (transcripts,
memory) keep one entry per agent.

The shim (`bin/cargo -> ferry`) is where builds are redirected. On every call
it resolves the agent from the working directory's git worktree: its root
must be exactly an agent's worktree (a host path, or `mnt/src` whose mounted
inode is one), so a separate worktree nested inside an agent's is not that
agent. For an agent it enters (or confirms it is already in) that agent's
sandbox, moves to the canonical path, and finds the workspace the call
selects (`cargo locate-project --workspace`, honouring `--manifest-path`;
skipped for a call at the worktree root without one):

| Workspace | Target dir |
|---|---|
| rooted at the worktree root | the fork |
| rooted at `<path>` inside the worktree | `<fork>/ferry-ws/<path>` |
| outside the worktree (`--manifest-path` into another agent's) | its own, as plain cargo; inherited target variables removed |

It sets `CARGO_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR` and the freshness env
accordingly and execs the real cargo (the next `cargo` on `PATH` that is not
ferry). Outside agent worktrees it execs the real cargo unchanged, except
inside a base's warm commands, where `FERRY_WARM_SRC` and `FERRY_WARM_TARGET`
apply the same table to the base checkout.

### `ferry rm NAME [--force]`

Refuses if processes still run for the agent (they hold `busy.lock`), if
another agent's worktree is nested inside its worktree (even with
`--force`), or, without `--force`, if the worktree has uncommitted changes or
commits that no branch, tag or remote reaches. With `--force` such commits
are kept under `refs/ferry/orphaned/<name>-<time>`. Removes the worktree and
fork; deletes the branch only if it still points at the agent's starting
commit and is not checked out elsewhere, otherwise keeps it and reports how
many commits it carries.

### `ferry gc [--dry-run]`

Reaps abandoned hook-created agents (clean, no new commits, no commits only
their worktree reaches, no ignored files beyond build output, idle, not
`git worktree lock`ed by their tool), then deletes bases that are neither
current nor referenced, skipping any base whose lock is held. It never runs
`git worktree prune`, which would also drop the user's own worktrees whose
directories are temporarily missing; Ferry unregisters only the worktrees it
removes.

## Fork backends

```rust
pub trait Backend {
    fn fork(&self, paths: &ForkPaths) -> Result<ForkStats, ForkError>;
    fn target_spec(&self, paths: &ForkPaths) -> TargetSpec;
    fn private_bytes(&self, paths: &ForkPaths) -> Result<u64, ForkError>;
}
```

| Backend | Platforms | Fork cost | Notes |
|---|---|---|---|
| `overlay` | Linux 5.11+, namespace sandbox required | O(1) | copy-up copies a file on its first write; on btrfs/XFS the copy-up is itself a reflink. The mount exists only inside the sandbox |
| `reflink` | Linux (btrfs, XFS, OpenZFS 2.2+), macOS (APFS) | per-file `FICLONE` (Linux), one `clonefile(2)` of the tree (macOS) | target is a real dir; mtimes preserved |
| `copy` | everywhere | full copy | fallback |

`auto` picks overlay when the agent runs in a namespace sandbox and overlayfs
works there, else reflink when the store filesystem supports it, else copy. An
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
   overlayfs (`lowerdir` = base target, `upperdir`, `workdir`, always
   `userxattr`), onto `mnt/target`.

Two overlay mounts must never use the same upper dir at once: the kernel only
warns, and the two views then diverge. Every process using an agent's overlay
therefore holds a shared `flock` on `agents/<name>/mount.lock` through an
inherited file descriptor. Under the agent lock, `enter` tries an exclusive
lock on it. A free lock is not proof that nothing uses the overlay: a process
in a nested namespace (`unshare`, `bwrap`) may have closed its descriptor and
still have the overlay mounted. So before mounting a new one, `enter` also
scans every readable `/proc/<pid>/mountinfo` for an overlay on the agent's
upper or work dir (one read per mount namespace and root, about 15 ms for 480
processes). Only when both checks come up empty does it mount and record the
namespace identity (and the upper and work dirs) in `ns.json`; otherwise it
`setns`es into a live process of the recorded namespace. An exiting session
holds the lock while the kernel unmounts its overlay, so `enter` re-checks
with backoff for up to 10 s before refusing, naming the processes that still
have the overlay mounted or hold the lock. `ferry list`, `rm` and gc count
those processes as users too.
Bind-mount-only sandboxes (reflink, copy) are safe to create per call.

Without namespaces (macOS; Linux with `sandbox = off` or where user
namespaces are blocked) builds run at host paths, and the first edit pays for
rustc's incremental state being keyed on absolute paths. Cargo's own unit
hashes survive the move (it hashes path packages relative to the workspace
root), but units can capture the base's absolute *source* paths, and
not always where Cargo looks: `env!("CARGO_MANIFEST_DIR")`, workspace-relative
`include_str!` on Cargo <= 1.100 and absolute `rerun-if-changed` paths show up
in dep-info, but a build script that writes its manifest dir into generated
code, or a proc macro that reads it from the environment (the `rust-embed`
and `include_dir` pattern), leaves no trace. Such a unit would stay "fresh"
while pointing at the base. So spawn deletes the fork's fingerprints of every
unit of every local package (those without a `source` in `Cargo.lock`), plus
any other unit whose dep-info names the base's source root, and drops the
nested workspaces' target dirs; registry and git dependencies keep their
reuse. Captured `OUT_DIR` paths are left alone: they point into the base's
immutable target dir, which outlives the agent. A seeded base refresh
without the namespace does the same against its seed.

The experimental remap mode (`[sandbox] remap-paths`, rustc 1.98+) recovers
incremental reuse without namespaces: builds reach the target dir through a
`.ferry-target` link inside the source root, and the `ferry-rustc`
`RUSTC_WORKSPACE_WRAPPER` adds `-Zremap-cwd-prefix=.` (with `RUSTC_BOOTSTRAP=1`
for workspace-member compiles only, and on non-nightly toolchains
`-Zallow-features=` so that `#![feature]` stays an error) and rewrites
absolute worktree and target paths in rustc's arguments to relative ones.

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

Locks are `flock(2)` on files in the store. Exclusive locks are never held
while an agent's build runs; the base lock is held for its own warm build:

| Lock | Held by | Duration |
|---|---|---|
| `<store>/lock` | spawn (registry step), rm, gc, setting `current` | milliseconds to seconds |
| `bases/<id>.lock` | `ferry base` for that id (exclusive), a refresh seeded from it (shared); gc only try-locks | the base build |
| `agents/<name>/lock` | spawn (whole), rm, sandbox entry for runs | spawn; ~1 ms per run |
| `agents/<name>/busy.lock` | shared by every `ferry run`, `shell` and shim call through an inherited fd; rm, list and gc try it exclusively | the life of the process and its children |
| `agents/<name>/mount.lock` | shared by every process using an overlay mount, as above | the life of the mount's users |

Lock order is store, then agent, then base. `ferry base` also takes the
store lock while holding a base lock (to set `current` before gc can see the
new base unreferenced); that is safe because gc only ever try-locks bases.

## Errors and output

Library code returns typed errors (`thiserror`); the CLI wraps them with
`anyhow` and prints `error:`, `caused by:` and a `hint:` from
`Error::hint()`. Data goes to stdout, progress to stderr, colour only on a
terminal. Every read command and every mutating command accepts `--json`,
including `doctor --apparmor-profile`.
