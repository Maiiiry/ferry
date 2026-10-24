# How it works

This chapter explains what Ferry does and why, at the level you need to use
it well. The [Architecture](architecture.md) chapter is the precise contract
the code is written against: invariants, on-disk layout, locks and lifecycle.

```text
 ferry base                 bases/<commit>-<fingerprint>/
 (once per commit)            src/     immutable checkout
                              target/  immutable warm build
                                 |
            +--------------------+--------------------+
            |                    |                    |
       ferry spawn a1       ferry spawn a2       ferry spawn a3
            |                    |                    |
       worktree a1          worktree a2          worktree a3     your branches, at host paths
       fork of target       fork of target       fork of target  reflink, overlay layer or copy
            |
       cargo for a1 (via `ferry run a1` or the shim) runs in a1's private
       mount namespace, where
            <store>/mnt/src     shows a1's worktree
            <store>/mnt/target  shows a1's fork
       Every agent builds at these same two paths, so the base's incremental
       state stays valid, and no agent can see another agent's build state.
```

## Bases

A base is a warm build of one commit, made by `ferry base`. Ferry checks the
commit out into its store (a locked git worktree), runs the warm commands
(by default `cargo build --workspace` and `cargo test --no-run --workspace`),
and records the result in `base.json`. From then on nothing writes to it.

A base is identified by its commit plus a fingerprint of everything that
would make its artifacts unusable: the `rustc -vV` and `cargo -V` output, the
commit's `rust-toolchain.toml`, the warm commands, the freshness mode, and
whether it was built in the namespace sandbox. Changing any of these makes
`ferry base` build a new base instead of reusing one. Agents keep the base
they were forked from until you remove them.

Refreshing is cheap: a new base for a later commit is seeded from the newest
ready base with the same fingerprint, so only what changed between the two
commits is rebuilt. Seeding needs the two commits to have identical
`Cargo.toml`s, `Cargo.lock`s, Cargo configs and toolchain files (and the same
`RUSTFLAGS`-style variables): a seeded target also holds units from older
commits, and a feature switched off and on again, or a dependency bump that
was reverted, would select one of those as if it were fresh. After such a
change the refresh is a cold build. `ferry base --cold` skips the seeding.

## Forks

`ferry spawn` gives an agent its own copy of the base's target dir without
copying it, where possible. There are three backends:

| Backend | How | Cost per agent |
|---|---|---|
| `reflink` | copy-on-write clone of every file (`FICLONE` on Linux, `clonefile(2)` on macOS) | time per file; disk only for files the agent rewrites |
| `overlay` | overlayfs: the base's target is a read-only lower layer, the agent writes to its own upper layer | constant time to fork; a file is copied into the upper layer on its first write (on btrfs and XFS that copy is itself a reflink) |
| `copy` | a full copy | time and disk for the whole target dir |

`auto` (the default) picks reflink when the store's filesystem supports
copy-on-write clones, else overlay when the agent runs in the namespace
sandbox and overlayfs works there, else copy. A reflink fork is a plain
directory, so a build needs no per-call overlay mount and does not wait for
the kernel to sync the whole store filesystem when its sandbox exits, which
costs an overlay build about 0.2 s. Ferry never uses hard links: Cargo
rewrites some files in place, so a hard-linked fork would let an agent's
build modify the base.

## Stamping: making the fork look fresh

A fresh git checkout gives every file a new modification time. Cargo compares
source mtimes with the time it last built each crate, so a forked target next
to a fresh checkout looks stale and Cargo rebuilds every workspace crate for
nothing. In the prototype that was 9 spurious crate rebuilds on ripgrep.

At spawn, Ferry sets the mtime of every tracked file to a fixed old time, but
only if the file is provably identical to the base's copy: same git blob,
same mode, same bytes on disk. Directories are stamped only if they list the
same entries as the base. Anything that differs keeps its real mtime and is
rebuilt. Stamping happens once, when the agent is created; Ferry never
re-stamps a live worktree, because that could hide an edit or a deletion.

On nightly toolchains Ferry uses Cargo's checksum freshness instead (see
[Configuration](configuration.md#freshness)), and still stamps, because build
script inputs are always compared by mtime.

## Canonical paths: keeping incremental state

Crate-level reuse is not the whole story. rustc's incremental cache records
absolute paths: the working directory and the output directory. When an agent
builds at a different path from the base, rustc throws that cache away and the
first edit recompiles each changed crate from scratch.

On Linux, Ferry runs each build in a private user and mount namespace. Inside
it, the agent's worktree is bind-mounted at `<store>/mnt/src` and its target
dir at `<store>/mnt/target`. The base was built at the same two paths, so
rustc finds its incremental state valid. In the prototype the first edit
after spawning took 1.75 s at the canonical path against 5.3 s at a different
path (ripgrep, 2 vCPU).

The namespace changes only those two mounts. Host paths keep working inside
it, your uid maps to itself (not root), and the mounts vanish when the last
process in the namespace exits. On the host, `<store>/mnt/src` and
`<store>/mnt/target` are empty directories.

Without namespaces (macOS, or Linux where they are blocked or `sandbox = "off"`),
agents build at their own paths. A crate can capture the base's absolute
paths in ways Cargo never sees: a build script that writes its
`CARGO_MANIFEST_DIR` into generated code, or a proc macro that reads it from
the environment (the `rust-embed` and `include_dir` pattern). Reusing such a
crate would make the agent read the base's files. So the agent's first build
recompiles every crate of the workspace and its path dependencies, while
registry and git dependencies stay reused. The first edit also loses rustc's
incremental state. The experimental [remap-paths mode](remap-paths.md)
recovers most of the incremental reuse, which also makes those recompiles
cheaper.

## Routing cargo: `ferry run` and the shim

Ferry has to intercept cargo to give it the right target dir and sandbox.
There are two ways in:

- **`ferry run <name> -- <command>`** enters the agent's sandbox and runs the
  command with Ferry's shim first on `PATH`. The working directory stays a
  host path. Use it for a single command or to start a whole agent.
- **The shim.** `ferry shim` prints a directory holding a `cargo` link to the
  ferry binary. With it first on `PATH`, every `cargo` call looks at its
  working directory. Inside an agent worktree it enters that agent's sandbox,
  moves to the canonical path, sets `CARGO_TARGET_DIR` and
  `CARGO_BUILD_BUILD_DIR` to the agent's fork, and runs the real cargo (the
  next `cargo` on `PATH` that is not Ferry). Anywhere else it runs the real
  cargo unchanged.

The agent is always derived from the working directory, never from the
environment. Agent tools copy their session environment into sub-agents that
work in other worktrees, so an exported `CARGO_TARGET_DIR` would point a
sub-agent at another agent's build state. That is the shared-target bug again,
which is why Ferry never exports it to anything but the cargo process it
starts.

## What each agent owns

Everything an agent's build writes lands in its own fork: overlay upper layer,
reflink clone or copy. Both the target dir and Cargo's build dir point at it,
so a `build.build-dir` in your Cargo config cannot redirect intermediates to a
shared place. The base is read-only to every backend. Removing an agent
removes its worktree and fork; the base stays until `ferry gc` finds it unused
and not current.

Each Cargo workspace gets its own target dir, because Cargo names a path
crate's artifacts after its path relative to the workspace root: two
workspaces sharing a target dir overwrite each other's crates of the same
name. The workspace at the worktree root uses the fork itself; one nested at
`<path>` (a `fuzz/` or `xtask/` workspace, say) uses `<fork>/ferry-ws/<path>`;
and a workspace outside the worktree, such as
`cargo test --manifest-path ../other-agent/Cargo.toml`, builds into its own
target dir, as it would without Ferry. Base warm commands get the same
mapping, so a nested workspace the base warmed is warm in agents too.

## Where things live

| What | Default (Linux) | Default (macOS) | Override |
|---|---|---|---|
| Build state: bases, forks, the shim | `$XDG_CACHE_HOME/ferry` or `~/.cache/ferry` | `~/Library/Caches/ferry` | `FERRY_HOME`, `[store] dir` |
| Agent worktrees (your uncommitted work) | `$XDG_DATA_HOME/ferry/worktrees` or `~/.local/share/ferry/worktrees` | `~/Library/Application Support/ferry/worktrees` | `FERRY_WORKTREES`, `[agent] worktree-dir`, `ferry spawn --path` |

Worktrees live outside the cache directory on purpose: they hold work you have
not committed, and caches get cleared. All build state Ferry writes is under
the one store directory, so an agent tool's sandbox needs a single writable
entry for it.
