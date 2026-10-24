# cargo-fleet: forkable warm Rust builds for parallel agents

Prototype + benchmarks, run on ripgrep @ 6892bc2 (39 crates, debug `cargo build`),
2 vCPU / 7 GB / ext4, rustc 1.97.0. Edit = new documented fn in `crates/ignore`
(forces `ignore` + `ripgrep` to rebuild). Small workspace on a small box, so read
the ratios and crate counts more than the absolute seconds.

## Headline numbers

| Scenario (new agent, then one edit)            | Time   | Crates compiled |
|------------------------------------------------|--------|-----------------|
| Cold worktree, fresh target                    | 17.1 s | 34              |
| Copied warm target, fresh checkout mtimes      | 7.2 s  | 9 (spurious)    |
| Copied warm target, stamped mtimes, diff path  | 5.3 s  | 2               |
| kache 1.2.2 (warm cache, new worktree)         | 5.5 s  | 2               |
| **cargo-fleet fork (stamped + canonical path)**| **1.75 s** | **2**       |

- Spawn with no edit: fork 0.10 s / 0 crates; kache 0.45 s (restores 34).
- Second edit onward: fork ~1.1 s, kache ~1.0 s. They converge once incremental state exists.
- 3 agents, 3 different edits, building concurrently: 2.7 s wall, each artifact contains
  only its own change, base untouched, `cargo test -p ignore` passes (190/190) in a fork.

## What actually matters (two separate fixes)

1. **Mtimes are the main enemy, not paths.** Cargo hashes workspace members by relative
   path, so a warm target copied to a different path is reusable. But fresh checkouts get
   new mtimes and Cargo's freshness check is mtime-based, so 9 crates rebuild for nothing.
   Stamping tracked files to a fixed mtime gives full crate-level reuse.
2. **Canonical path preserves incremental.** rustc's incremental cache embeds absolute
   paths. Same fork at a different path: 5.3 s first edit. Bind-mounted at `/work` in a
   private mount namespace: 1.75 s. ~3x, consistent across 3 runs.

## Correctness bug in the naive approach

Two worktrees sharing one `CARGO_TARGET_DIR`: agent B edits, agent A builds, then B builds.
B's source is older than A's fresh artifact, so Cargo marks it fresh and **B gets A's code
with 0 crates rebuilt**. Reproduced and verified by inspecting the rlib (`ONLY_IN_A` in B's
build). Shared target dirs across agents are not just slow, they're silently wrong.

## vs kache

kache (content-addressed RUSTC_WRAPPER, path-normalized keys) is the closest OSS project
and is great at cross-worktree / cross-machine artifact reuse. Its gap for agents is the
first edit after spawn: restored artifacts carry no incremental state. These compose well:
kache to build the base fast across machines and commits, fork for per-agent incremental.

## Costs / open issues

- Overlay copy-up copies whole files: each agent's upper layer was ~190 MB (base 536 MB),
  mostly the relinked debug binary and incremental session dirs. Next: lower layer on a
  reflink fs, `split-debuginfo`, or a FUSE layer with block-level CoW.
- Needs root or user namespaces (`unshare -m`); Linux only. macOS: stamped mtimes + APFS
  `clonefile` gets the crate-level win but not the incremental one.
- Mtime stamping is a hack. `-Zchecksum-freshness` (nightly) would replace it and also
  close the shared-target bug class. Worth tracking.
- Lock waits seen under concurrency were only on the shared `~/.cargo` package cache.
- rust-analyzer per worktree not addressed yet.

## Usage

```
cargo-fleet base            # build immutable warm base for HEAD
cargo-fleet spawn agent-1   # worktree + zero-copy target fork
cargo-fleet shell agent-1   # sandbox shell at /work; point the agent here
cargo-fleet run agent-1 cargo test
cargo-fleet rm agent-1; cargo-fleet gc
```
