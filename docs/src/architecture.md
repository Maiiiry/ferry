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
   through a link by its canonical path when that lies inside the input's
   package or the target dir, but follows a build script's
   `rerun-if-changed` path through it). Checksum freshness compares content
   instead, so the units that read through such a link (one with an end in
   the checkout) also get their dep-info invalidated (see
   [Freshness modes](#freshness-modes)). A
   directory is stamped only if its entry names equal the base checkout's
   directory, because Cargo's `rerun-if-changed=<dir>` takes the recursive
   max mtime of the directory, its subdirectories and its files, and a
   deletion is only visible as a directory mtime change. `StampPlan` can only
   be built by the verifying function. The one other stamping function,
   `stamp_base_checkout`, stamps a fresh, unseeded base checkout, which is
   the reference itself; only `base::prepare` calls it. Every file and
   directory that is *not* stamped gets an mtime no older than the base
   (`freshen_skipped`), so a hook that restores commit times or a clock that
   went backwards cannot make it look fresh. Stamping happens once, at spawn;
   a live agent tree is never re-stamped.
2. **Bases are immutable once built** (`base`). A base is keyed by commit plus
   a fingerprint of toolchain, warm commands and modes. `base.json` is written
   last, atomically, and doubles as the "ready" marker; nothing writes to a
   ready base. Refreshing builds a new base; `gc` removes a base only when it
   is not current and no agent references it. A ready base's target holds
   only units its own warm build compiled or found fresh (`prune`), so every
   unit an agent or a later refresh reuses was checked against the content
   stamping compares. There are three ways to get there: a cold build; a
   refresh seeded from a base with the same *unit inputs* (every manifest,
   lockfile, Cargo config and toolchain file of the commit, the Cargo configs
   above the store, Cargo's flag variables, `CARGO_INCREMENTAL`, whether
   `CI` or `TF_BUILD` is set, and a version of this list, so that no base
   recorded under an older list matches), which selects the same units; and a refresh
   seeded across unit inputs from a base whose warm build was observed,
   after which every unit the build neither wrote nor reported is deleted.
   When that observation is incomplete, the target is emptied and built
   again from scratch rather than published.
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
  freshness.rs   stamping plan + application, freshness mode; relocation.rs:
                 what a fork built at other paths must rebuild
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
    watch.lock                     held by `ferry base --watch`
    mnt/src  mnt/target            canonical mountpoints (empty on the host)
    bases/<commit>-<fp>/           immutable once base.json exists
      base.json  src/  target/
      relocation.json              built without the sandbox: what forks remove from target/
      warm-units/                  the shim's records of warm cargo calls, while building
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
3. `git worktree add --detach` the checkout and `git worktree lock` it. Unless
   `--cold` is given, seed from the newest ready base with the same
   fingerprint at another commit that has the same unit inputs or (with
   `[base] prune`, the default) was observed: clone its target (reflink when
   possible), relocate the clone without the sandbox (see below), stamp only
   files proven identical to its checkout, and invalidate the units that read
   through a symlink retargeted within, into or out of the checkout, exactly
   as for an agent, so the warm build only rebuilds what changed. Otherwise
   stamp all tracked files and directories (the base is the reference,
   nothing to compare). A seed with other unit inputs loses its nested
   workspaces' target dirs (`ferry-ws/`), which are not observed.
4. Create `warm-units/` and take a write threshold: the mtime of a file
   created there that is strictly newer than one created just before, so
   every file written later is at least as new and every cloned file older.
5. Enter the sandbox (irreversible for this process; host paths stay
   reachable), run each warm command with `sh -c`, stdout redirected to
   stderr, the shim first on `PATH` so each workspace gets the target dir it
   gets in agents, and `CARGO_BUILD_WARNINGS=warn`. A failing command
   discards the base, except a generated clippy command, which only warns
   and is recorded in `incomplete`. Clippy commands are left out (before the
   base id is computed) when `cargo clippy --version` fails. The shim records
   each build-type cargo call (`build`, `check`, `test`, `run`, `clippy`,
   `doc`, ...) in a file of its own under `warm-units/`: it adds
   `--message-format=json-render-diagnostics` unless the call asks for JSON
   itself, records every `compiler-artifact` and `build-script-executed`
   message with the inode, length and mtime of each named file as it
   arrives, and passes all other output through (and the messages too, when
   the call asked for them). A call asking for `human` or `short` messages,
   or a subcommand it does not know (`nextest`, aliases, `install`), is
   recorded as unobservable.
6. Work out which units the build used. Every profile dir (a directory with
   a `.cargo-lock`) is scanned in both build-dir layouts; a unit is used if
   Cargo wrote its fingerprint after the threshold, or if a message names it:
   by a path inside its own directory or carrying its hash, or, for files
   uplifted without a hash (binaries, examples, root proc macros and
   dylibs), as the only unit of that package with the fingerprint file Cargo
   names after the target, else the one whose output has the recorded inode
   (Linux hard links) or length and mtime (macOS copies). The observation is
   incomplete if a call was unobservable or did not finish, a message
   matches no unit or several, a rebuilt unit's fingerprint predates the
   threshold, a written unit was reported by no message (a cargo call that
   bypassed the shim) although every call succeeded, a directory in the
   target cannot be read, or a profile dir has an unknown layout. After a
   call that failed (a clippy command under `--keep-going`, say), a written
   unit no message reported is one that failed to build (Cargo writes its
   fingerprint dir, with an empty hash, and reports nothing), and it counts
   as unused.
7. With other unit inputs than the seed: if the observation is complete,
   delete every unused unit (fingerprint, build dir, hash-named outputs, and
   hash-named outputs without a fingerprint), leaving uplifted files, `doc/`
   and `incremental/`; otherwise warn, empty the target, and run the warm
   commands again into it without a seed. Record `observed` and `pruned`.
8. Record files the build wrote into the source tree (build scripts writing
   outside `OUT_DIR`) as warnings; without the sandbox, compute the
   relocation plan and write `relocation.json` (see below); write
   `base.json`; set `current` before releasing the base lock.

### `ferry base --watch [--rev R] [--interval S] [--gc]`

A foreground poller. It refuses an `R` that names no commit, takes
`<store>/watch.lock` (a second watcher is refused, naming the holder), then
every `S` seconds (default 2) resolves `R` with one `git rev-parse`,
skipping polls while the watcher's worktree is in a rebase, merge,
cherry-pick, revert or bisect. A commit seen on two polls
in a row that is neither the current base's commit at startup nor the last
one built or failed is built by a child `ferry --json base --rev <commit>`
(building enters the sandbox for the rest of the process, and the child
re-reads `ferry.toml`); its stderr is the watcher's. A refresh is never
interrupted when `R` moves: the newest commit is built after it. A failed
build leaves the current base alone and is not retried until `R` moves.
`--gc` runs gc in the watcher after each new base.

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
   the namespace sandbox, apply the base's relocation plan (see below). If a
   tracked symlink was retargeted within, into or out of the checkout,
   invalidate the units that read through it (see
   [Freshness modes](#freshness-modes)). Write
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
apply the same table to the base checkout, `FERRY_WARM_CARGO` holds the
base toolchain's `cargo -V`, and `FERRY_WARM_UNITS` names the directory the
shim records the call in (see `ferry base`, step 5).

With `CI` or `TF_BUILD` set, Cargo 1.99 and later turn incremental
compilation off after merging profiles, so ahead of `[profile.*]
incremental = true`, and the switch enters every unit's hash: an agent with
`CI` would rebuild every workspace member of a base warmed without it. So
wherever the shim sets a target dir, it also sets `CARGO_INCREMENTAL=1` when
either variable is set and the base's toolchain is Cargo 1.99 or later,
unless the call has one of the settings Cargo ranks above the switch:
`CARGO_INCREMENTAL`, `CARGO_BUILD_INCREMENTAL`, or `build.incremental` in a
`--config` value or in a `.cargo/config{,.toml}` in the working directory's
ancestors or Cargo home (`include`s are not followed). `CI` and `TF_BUILD`
are passed on unchanged. Processes cargo starts inherit
`CARGO_INCREMENTAL=1`, so a cargo call they make on a workspace outside the
checkout builds incrementally too. Release and bench units still depend on
`CI`: by default they are incremental with it and not without. So whether
`CI` or `TF_BUILD` is set is a unit input (invariant 2). The costs are
listed under
[Cargo configuration pitfalls](troubleshooting.md#cargo-configuration-pitfalls).

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
| `reflink` | Linux (btrfs, XFS, OpenZFS 2.2+), macOS (APFS) | per-file `FICLONE` (Linux), one `clonefile(2)` of the tree (macOS) | target is a real dir; mtimes preserved |
| `overlay` | Linux 5.11+, namespace sandbox required | O(1) | copy-up copies a file on its first write; on btrfs/XFS the copy-up is itself a reflink. The mount exists only inside the sandbox |
| `copy` | everywhere | full copy | fallback |

`auto` picks reflink when the store filesystem supports copy-on-write clones,
else overlay when the agent runs in a namespace sandbox and overlayfs works
there, else copy. So auto picks overlay only on filesystems without clones
(ext4, tmpfs). Reflink comes first because the overlay costs every build
outside Cargo: the mount protocol below on entry, and on the per-call path
(shim, hooks) a `syncfs` of the whole upper filesystem when the last process
using the mount exits. An agent keeps the backend recorded at spawn. An
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
root), but a unit can capture the base's absolute paths where Cargo never
looks again: a build script writing its manifest dir into generated code or
a `rustc-env` value, a proc macro reading it from the environment (the
`rust-embed` pattern), a test that bakes in `env!("CARGO_BIN_EXE_app")`. Such
a unit would stay "fresh" while pointing at the base. So the fork gets a
*relocation plan* (`freshness::plan_relocation`): it keeps every unit unless
something recorded in the target shows an old path, and removes the rest.

| Rule | Reads | Flags a unit when |
|---|---|---|
| R1 | its `dep-*` and fingerprint JSON | they name any old path (a workspace file outside the package on Cargo <= 1.100, an absolute `rerun-if-changed`, the base checkout's git dir) |
| R2 | rustc's dep-info | an `env!` value or an absolute input names a checkout path (a vendored crate, or a path package outside the workspace root, rebuilds anyway, because Cargo hashes its absolute source path) |
| R3 | `env!` values in rustc's dep-info | one names a target path that is not exempt (`CARGO_BIN_EXE_*`, `CARGO_TARGET_TMPDIR`, a profile dir) |
| R4 | a build script's directives | they name an old path other than its own `OUT_DIR` spelled as in `root-output`, the string Cargo rewrites when it replays them; or a link argument (an rpath) names a dead `OUT_DIR`, which the linker already baked into the package's binaries |
| R5 | every file and symlink in an `OUT_DIR` | one names an old path that is not exempt, or cannot be read |
| R6 | generated files outside `OUT_DIR`s that a compile included | one names an old path that is not exempt |
| R7 | proc-macro libraries, local macros' sources | it can invoke a proc macro that may read a path from the environment |

Old paths are found with one needle per root, the longest suffix of the store
path and of the base checkout's git dir made only of bytes that no escaping
rewrites (`[A-Za-z0-9._/-]`), so every spelling (Debug, JSON, Make, percent
encoded) contains it; what follows the needle tells a checkout from a target
dir, and `<base src>/.ferry-target` (remap mode) counts as the target.

Whether a target path may stay depends on whether that target is *live*. A
base built without the namespace is immutable and gc keeps it while an agent
references it, so a unit may keep a path into the `OUT_DIR` of a build-script
run it depends on: the base's bytes there equal the fork's until that run
reruns, and then the unit rebuilds in the same build. Every other target path
is *dead*: a namespace base's `mnt/target` (an empty mountpoint on the host),
and the target of a seed, which gc may collect. A dead path is exempt only in
a script's own directives other than link arguments, where Cargo rewrites the
exact `root-output` string, and where a registry crate includes from its own
`OUT_DIR` at compile time (serde's `include!(concat!(env!("OUT_DIR"), ...))`).

A removed compile unit loses its fingerprint dir, so its dependents go stale
in the same build. Every build-script run that depends on a removed unit (a
build-dependency, a `links` upstream) is removed whole, run dir and `OUT_DIR`
included, so it reruns from an empty `OUT_DIR` instead of over the base's
state, and Cargo rebuilds what depends on it in the same build; removing only
a run's fingerprint would leave its dependents one build behind. Nested
workspaces' target dirs (`ferry-ws/`) are planned like the rest and kept.
What cannot be read or parsed is removed, so a new Cargo fingerprint format
costs a cold build, never a wrong one; a store path without a plain suffix of
8 bytes empties the fork.

`ferry base` computes the plan of a base built without the namespace before
writing `base.json` and stores it as `relocation.json`, so a spawn only
deletes what it lists (it recomputes the plan after a Ferry upgrade). Files
are scanned a megabyte at a time, so a library of gigabytes in an `OUT_DIR`
costs time, not memory. For a namespace base, spawn computes the plan on the
base's target, with every target path dead. A seeded refresh without the
namespace applies a dead plan to its clone of the seed's target before the
warm build, so the new base never depends on the seed's.

One rule is a heuristic. R7 assumes that a registry proc macro reading a path
from the environment names the variable literally (`CARGO_MANIFEST_DIR`,
`OUT_DIR`, `PWD` and the other path variables), which holds for every
path-embedding crate surveyed (rust-embed, rstest, manifest-dir-macros).
Crates on a small allowlist whose hits a human checked (`document-features`
0.2.12, `xflags-macros` 0.3.2) are exempt, by exact version. A local proc
macro also counts when its sources use the working directory, a subprocess or
dynamic variable names. What the plan cannot see:

- paths stored encoded: compressed, base64, hashed, UTF-16, or JSON with `\/`;
- registry proc macros that derive a path from the working directory, a
  subprocess (`cargo metadata`, `git rev-parse`) or a variable name built at
  run time, without naming a listed variable;
- behaviour that depends on location without recording a path: existence
  checks, or reads through `../..` out of the worktree;
- paths that a program derives at run time from an exempt live `OUT_DIR`
  with `..`, such as `Path::new(env!("OUT_DIR")).join("../../../helper")`,
  which leads to the base's binary instead of the agent's;
- path variables outside the list read by proc macros, such as `[env]`
  entries with `relative = true`;
- a registry crate that includes from a dead `OUT_DIR` and also reads it at
  run time;
- writes outside `OUT_DIR` that no compile includes (`target/cxxbridge`
  links, `$HOME`, `/tmp`); the source tree is reported as `dirtied`;
- relative paths between the source and target trees, and absolute paths
  outside the store and git dir that an agent would compute differently;
- debuginfo, panic locations, cached diagnostics and doc output (cosmetic);
- run-time writes through a live `OUT_DIR` path, which modify the base;
- literals that a proc macro built with optimizations only compares against.

Staleness that the namespace sandbox has too (a git hash baked in without
`rerun-if-changed`, a macro reading a file Cargo does not track) is not
relocation's to fix.

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

Leaving a file unstamped cannot show checksum freshness a symlink that leads
elsewhere than in the base. When the canonical path of a rustc input read
through a link lies inside the input's package or the target dir, Cargo
records the input by that path (otherwise by the path through the link) and
compares the content of the base's file there, which the agent has too. So
when a tracked symlink leads to another place in the checkout than in the
base, or into or out of it, spawn (and a seeded refresh, against its seed)
also empties the Cargo dep-info file (`dep-*` in the fingerprint dir, written
to an overlay fork's upper dir) of these units (`freshness::link_rebuilds`):

- each compile unit whose rustc dep-info names an input in the checkout that
  resolves differently in the two checkouts, unless it resolves into the
  store in both (remap mode's `.ferry-target` link, below which Cargo tracks
  paths relative to the target dir);
- each compile unit without readable rustc dep-info, except rustdoc's, which
  compares mtimes.

Build-script runs compare mtimes too, so leaving their inputs unstamped is
enough. Cargo cannot parse an emptied file, so it rebuilds the unit and its
dependents in the same build.

## Concurrency

Locks are `flock(2)` on files in the store. Exclusive locks are never held
while an agent's build runs; the base lock is held for its own warm build:

| Lock | Held by | Duration |
|---|---|---|
| `<store>/lock` | spawn (registry step), rm, gc, setting `current` | milliseconds to seconds |
| `bases/<id>.lock` | `ferry base` for that id (exclusive), a refresh seeded from it (shared); gc and spawn only try-lock | the base build |
| `<store>/watch.lock` | `ferry base --watch` (try-locked) | the life of the watcher |
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
