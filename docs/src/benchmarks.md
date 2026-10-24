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

### ripgrep, Ferry at `d7fb5cb`

| | |
|---|---|
| Workload | ripgrep at `6892bc2`, debug `cargo build`: 44 units (34 crates; each build script counts as its own unit) |
| Edit | appends a documented `pub fn` to `crates/ignore/src/lib.rs`, which forces `ignore` and `rg` to rebuild; every run uses a new function |
| CPU | AMD Ryzen 5 3600X (6 cores, 12 logical CPUs, governor powersave) |
| RAM | 31 GiB |
| OS, kernel | NixOS 26.11, Linux 6.18.55 |
| Filesystem | btrfs (zstd:1 compression), which holds the store and the work dirs |
| Toolchain | rustc 1.99.0, cargo 1.99.0 |
| Other tools | kache 1.2.2, hyperfine 1.21.0 |
| Runs | 3 per step; 2 for steps that are, or prepare with, a cold build |
| Date (UTC) | 2026-10-24, 20:53 to 21:01 |
| Load average | 1.21 before, 4.86 after (1 min), on 12 logical CPUs: a desktop in use, not an idle machine |

First edit in a new agent (spawn, edit, build). Every artifact check passed:
the edited crate's rlib held exactly that agent's own edit.

| Scenario | Median | Min | Max | Units compiled |
|---|---:|---:|---:|---:|
| Cold worktree, empty target | 7.063 s | 7.019 s | 7.107 s | 44 |
| Copied warm target (`cp -a`), fresh checkout | 3.675 s | 3.598 s | 3.687 s | 10 |
| kache 1.2.2, warm local store | 2.863 s | 2.836 s | 2.876 s | 10, of which 8 restored from its store |
| Ferry, sandbox off | 3.204 s | 3.107 s | 3.480 s | 2 |
| Ferry, sandbox off, [remap-paths](remap-paths.md) | 2.000 s | 1.975 s | 2.033 s | 2 |
| Ferry, namespace sandbox, overlay backend | 1.252 s | 1.251 s | 1.274 s | 2 |
| Ferry, namespace sandbox, reflink backend | 1.130 s | 1.126 s | 1.137 s | 2 |

Other steps from the same run (medians):

| Step | Result |
|---|---|
| `ferry spawn` | 0.103 s (overlay), 0.156 s (reflink), 0.169 s (sandbox off) |
| First build after spawn, no edit | 0.06–0.08 s, 0 units compiled, in every Ferry scenario |
| 3 agents with different edits building at once (wall) | 1.409 s (reflink), 1.572 s (overlay), 2.753 s (remap-paths), 4.130 s (sandbox off); 6.663 s for 3 worktrees sharing one `CARGO_TARGET_DIR` |
| Second, different edit in the same agent | 0.878 s (reflink), 1.066 s (overlay), 0.890 s (remap-paths), 0.940 s (sandbox off), 0.912 s (kache) |
| `ferry base` after a commit, seeded from the previous base | 1.338 s, 2 units |
| `ferry base --cold` after the same commit | 7.414 s, 44 units |

What these numbers show:

- **The gain is in a new agent's first edits.** Later edits take about the
  same time with kache and in every Ferry scenario.
- **The overlay backend is slower than reflink on later edits** (1.07 s
  against 0.88 s). Every timed build here is its own `ferry run`, and an
  overlay build waits for its sandbox's teardown, about 0.2 s per build on
  ripgrep (see [How it works](how-it-works.md)). That matches the gap.
- **Sandbox off loses the incremental state.** The agent reuses every
  unchanged crate, but `ignore` and `rg` recompile from scratch. kache
  compiles the same two (the table's 10 counts the 8 it restores from its
  store, which Cargo reports as compiled), and is faster here.
  [Remap-paths](remap-paths.md#measured-results) keeps most of the
  library's incremental state at the agent's own path; on Linux the `rg`
  binary, built with debug info, still recompiles from scratch.
- **These are one workspace's results.** This page does not scale them to
  larger workspaces. rust-analyzer and macOS results are not published yet.

All steps, the disk usage, and the raw run times are in
[`bench/results/ripgrep/`](https://github.com/patwrall/ferry/tree/main/bench/results/ripgrep)
(`results.md`, `results.json`, `env.json`, and hyperfine's exports).

## CI runs (macOS)

macOS has no namespace sandbox, so its numbers come from a reduced benchmark
that the `Bench` workflow runs on a GitHub-hosted macOS runner when started
by hand (see `bench/README.md`, "CI runs"). These runners are shared virtual
machines. Their results are published in their own table, never next to the
results above, and each table states the runner label, image version, run
link and date; the Ferry commit, rustc, Xcode and linker versions; the runs
per step with the minimum and maximum of each step; and the load before and
after. Every such table carries this note, with the hardware taken from that
run's `env.json` (`logical_cpus`, `cpu_model` and `ram_bytes`), because it
depends on the runner label:

> Measured on a shared GitHub-hosted VM (_logical CPUs_ vCPU _CPU model_,
> _RAM_ GiB). Not comparable with the local results above or with other CI
> runs; ratios within one run are indicative, absolute seconds are not.

Claims about how much of rustc's incremental state survives on macOS come
from the test suite's codegen-unit counts, which do not depend on timing
noise, and not from these times.

No CI results have been published yet.

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
