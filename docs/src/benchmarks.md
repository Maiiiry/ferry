# Benchmarks

Ferry's benchmark harness is in `bench/` in the repository. Results are
published here as they are measured. This page never shows estimated or
extrapolated numbers: every figure comes from a recorded run.

## What is measured

| Metric | What it is |
|---|---|
| Spawn time | from `ferry spawn` (or the equivalent setup) until the agent can build |
| First edit after spawn | a fixed source edit in a new agent, then a build; wall time and crates compiled |
| Steady-state edit | the next edit in the same agent, once it has its own incremental state |
| Disk per agent | bytes the agent holds beyond the base after the edits (`ferry list` `DISK`) |
| N concurrent agents | N agents making different edits and building at the same time: wall time, and whether each artifact contains only its own change |

Every result records the hardware (CPU model and count, memory), the OS and
kernel, the filesystem of the store, the Rust toolchain, the workload and its
commit, and the versions of the other tools compared. Numbers from one
machine are not projected onto another, and results from small workloads are
not scaled up to large ones.

## Results

<!-- BENCH:RESULTS -->

No results from the Ferry binary have been published yet.

## Prototype results

Before Ferry was written, a bash prototype tested the approach. These are its
measured results: **prototype (bash) on ripgrep, 2 vCPU**. Setup: ripgrep at
commit 6892bc2 (39 crates, debug `cargo build`), 2 vCPU, 7 GB RAM, ext4,
rustc 1.97.0. The edit adds a documented function to `crates/ignore`, which
forces `ignore` and `ripgrep` to rebuild. It is a small workspace on a small
machine, so the ratios and crate counts say more than the absolute times.

| Scenario (new agent, then one edit) | Time | Crates compiled |
|---|---|---|
| Cold worktree, fresh target | 17.1 s | 34 |
| Copied warm target, fresh checkout mtimes | 7.2 s | 9 (spurious) |
| Copied warm target, stamped mtimes, different path | 5.3 s | 2 |
| kache 1.2.2 (warm cache, new worktree) | 5.5 s | 2 |
| Prototype fork (stamped mtimes, canonical path) | 1.75 s | 2 |

The prototype also measured:

- spawning with no edit: 0.10 s and 0 crates for the fork, 0.45 s for kache
  (which restores 34 crates);
- from the second edit on: about 1.1 s for the fork and 1.0 s for kache,
  which converge once the agent has its own incremental state;
- 3 agents making 3 different edits and building concurrently: 2.7 s wall
  time, each artifact containing only its own change, the base untouched.
