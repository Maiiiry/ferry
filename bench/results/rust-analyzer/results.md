# Ferry benchmark: rust-analyzer

- Workspace: https://github.com/rust-lang/rust-analyzer at `0c4d74686b21f2b3bd8872b6d3cf36ad2e740d49`
- Edit: append a documented `pub fn` to `crates/ide-db/src/lib.rs` (package `ide-db`); every run uses a new, unique function
- Build command: `cargo build` (debug), JSON messages; crates = `compiler-artifact` messages with `"fresh": false`
- Runs per step: 3 (steps that are or prepare with a cold build: 2); concurrent agents: 3

## Environment

| | |
|---|---|
| CPU | AMD Ryzen 5 3600X 6-Core Processor (6 cores, 12 logical CPUs, governor powersave) |
| RAM | 31 GiB; available at start 18614.1 MiB, swap in use at start 745.2 MiB |
| OS / kernel | NixOS 26.11 (Zokor) / 6.18.55 |
| Bench filesystem | btrfs (`rw,noatime,nodiratime,compress=zstd:1,ssd,discard=async,space_cache=v2,subvolid=257,subvol=/@home`) |
| rustc | `rustc 1.99.0 (b940084d7 2026-09-28)` |
| cargo | `cargo 1.99.0 (5f94df478 2026-08-27)` |
| ferry | `ferry 0.1.2` at `5be5ee14e85e614b6b2e857d71bfa234ae089ef2` |
| kache | `kache 1.2.2` |
| hyperfine | `hyperfine 1.21.0` |
| Date (UTC) | 2026-10-27T02:06:47Z to 2026-10-27T05:21:22Z |
| Load average (1/5/15 min) | before 4.28 3.86 3.04, after 8.80 8.13 7.84 |

## First edit after spawning a new agent

A new agent is created, one edit is applied, and the timed build is the first build it runs.

| Scenario | Median | Mean | Min | Max | Runs | Crates compiled | Artifact check |
|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | 73.978 s | 73.978 s | 73.550 s | 74.406 s | 2 | 281, 281 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | 67.000 s | 68.199 s | 62.478 s | 75.118 s | 3 | 51, 51, 51 | ok |
| kache, warm local store | 26.026 s | 26.045 s | 25.772 s | 26.338 s | 3 | 58, 58, 58 | ok |
| Ferry, sandbox off (crate-level reuse only) | 29.116 s | 36.428 s | 28.811 s | 51.358 s | 3 | 12, 12, 12 | ok |
| Ferry, remap-paths (sandbox off) | 9.576 s | 10.061 s | 9.359 s | 11.247 s | 3 | 12, 12, 12 | ok |
| Ferry, overlay backend | 7.384 s | 7.315 s | 7.175 s | 7.387 s | 3 | 9, 9, 9 | ok |
| Ferry, reflink backend | 7.687 s | 7.615 s | 7.382 s | 7.777 s | 3 | 9, 9, 9 | ok |

## All timed steps

| Scenario | Step | Median | Mean | Min | Max | Runs | Crates compiled (per run) | Check |
|---|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | first build after spawn, no edit | 91.922 s | 91.922 s | 74.078 s | 109.767 s | 2 | 281, 281 | ok |
| Cold worktree, empty target | **first edit after spawn** (spawn, edit, build) | 73.978 s | 73.978 s | 73.550 s | 74.406 s | 2 | 281, 281 | ok |
| Cold worktree, empty target | first edit after a spawn build (spawn, build, edit, build) | 7.251 s | 7.251 s | 7.193 s | 7.310 s | 2 | 9, 9 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first build after spawn, no edit | 61.629 s | 62.074 s | 60.220 s | 64.372 s | 3 | 51, 51, 51 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | **first edit after spawn** (spawn, edit, build) | 67.000 s | 68.199 s | 62.478 s | 75.118 s | 3 | 51, 51, 51 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first edit after a spawn build (spawn, build, edit, build) | 7.379 s | 7.469 s | 7.152 s | 7.876 s | 3 | 9, 9, 9 | ok |
| Shared `CARGO_TARGET_DIR` | agents with different edits building at once (wall) | 9.815 s | 9.822 s | 8.899 s | 10.752 s | 3 | –, –, – | – |
| kache, warm local store | first build after spawn, no edit | 2.897 s | 9.846 s | 2.591 s | 24.051 s | 3 | 58, 58, 58 | ok |
| kache, warm local store | **first edit after spawn** (spawn, edit, build) | 26.026 s | 26.045 s | 25.772 s | 26.338 s | 3 | 58, 58, 58 | ok |
| kache, warm local store | first edit after a spawn build (spawn, build, edit, build) | 26.207 s | 26.313 s | 25.935 s | 26.797 s | 3 | 9, 9, 9 | ok |
| kache, warm local store | second, different edit (steady state) | 7.400 s | 7.464 s | 7.320 s | 7.673 s | 3 | 9, 9, 9 | ok |
| Ferry, sandbox off (crate-level reuse only) | `ferry base` (cold) | 89.285 s | 89.285 s | 74.400 s | 104.170 s | 2 | 280, 278 | – |
| Ferry, sandbox off (crate-level reuse only) | `ferry spawn` | 1.378 s | 1.380 s | 1.208 s | 1.554 s | 3 | –, –, – | – |
| Ferry, sandbox off (crate-level reuse only) | first build after spawn, no edit | 27.314 s | 27.762 s | 26.393 s | 29.579 s | 3 | 12, 12, 12 | ok |
| Ferry, sandbox off (crate-level reuse only) | **first edit after spawn** (spawn, edit, build) | 29.116 s | 36.428 s | 28.811 s | 51.358 s | 3 | 12, 12, 12 | ok |
| Ferry, sandbox off (crate-level reuse only) | first edit after a spawn build (spawn, build, edit, build) | 7.152 s | 7.131 s | 7.088 s | 7.153 s | 3 | 9, 9, 9 | ok |
| Ferry, sandbox off (crate-level reuse only) | second, different edit (steady state) | 7.439 s | 7.748 s | 7.276 s | 8.527 s | 3 | 9, 9, 9 | ok |
| Ferry, sandbox off (crate-level reuse only) | agents with different edits building at once (wall) | 41.808 s | 50.783 s | 41.326 s | 69.217 s | 3 | –, –, – | ok |
| Ferry, remap-paths (sandbox off) | `ferry base` (cold) | 89.569 s | 89.569 s | 75.197 s | 103.941 s | 2 | 281, 279 | – |
| Ferry, remap-paths (sandbox off) | `ferry spawn` | 1.100 s | 1.217 s | 1.065 s | 1.486 s | 3 | –, –, – | – |
| Ferry, remap-paths (sandbox off) | first build after spawn, no edit | 9.269 s | 9.211 s | 8.865 s | 9.497 s | 3 | 12, 12, 12 | ok |
| Ferry, remap-paths (sandbox off) | **first edit after spawn** (spawn, edit, build) | 9.576 s | 10.061 s | 9.359 s | 11.247 s | 3 | 12, 12, 12 | ok |
| Ferry, remap-paths (sandbox off) | first edit after a spawn build (spawn, build, edit, build) | 7.589 s | 7.651 s | 7.430 s | 7.935 s | 3 | 9, 9, 9 | ok |
| Ferry, remap-paths (sandbox off) | second, different edit (steady state) | 7.730 s | 7.690 s | 7.539 s | 7.801 s | 3 | 9, 9, 9 | ok |
| Ferry, remap-paths (sandbox off) | agents with different edits building at once (wall) | 16.417 s | 26.233 s | 15.317 s | 46.966 s | 3 | –, –, – | ok |
| Ferry, overlay backend | `ferry base` (cold) | 3779.752 s | 3779.752 s | 109.221 s | 7450.282 s | 2 | 278, 281 | – |
| Ferry, overlay backend | `ferry spawn` | 0.448 s | 0.470 s | 0.434 s | 0.529 s | 3 | –, –, – | – |
| Ferry, overlay backend | first build after spawn, no edit | 0.241 s | 0.294 s | 0.240 s | 0.400 s | 3 | 0, 0, 0 | ok |
| Ferry, overlay backend | **first edit after spawn** (spawn, edit, build) | 7.384 s | 7.315 s | 7.175 s | 7.387 s | 3 | 9, 9, 9 | ok |
| Ferry, overlay backend | first edit after a spawn build (spawn, build, edit, build) | 7.490 s | 7.442 s | 7.199 s | 7.637 s | 3 | 9, 9, 9 | ok |
| Ferry, overlay backend | second, different edit (steady state) | 7.293 s | 21.660 s | 6.958 s | 50.728 s | 3 | 9, 9, 9 | ok |
| Ferry, overlay backend | agents with different edits building at once (wall) | 10.304 s | 10.352 s | 10.091 s | 10.661 s | 3 | –, –, – | ok |
| Ferry, reflink backend | `ferry spawn` | 1.277 s | 1.196 s | 1.033 s | 1.279 s | 3 | –, –, – | – |
| Ferry, reflink backend | first build after spawn, no edit | 0.215 s | 0.213 s | 0.207 s | 0.216 s | 3 | 0, 0, 0 | ok |
| Ferry, reflink backend | **first edit after spawn** (spawn, edit, build) | 7.687 s | 7.615 s | 7.382 s | 7.777 s | 3 | 9, 9, 9 | ok |
| Ferry, reflink backend | first edit after a spawn build (spawn, build, edit, build) | 7.596 s | 7.559 s | 7.342 s | 7.740 s | 3 | 9, 9, 9 | ok |
| Ferry, reflink backend | second, different edit (steady state) | 6.585 s | 6.587 s | 6.400 s | 6.776 s | 3 | 9, 9, 9 | ok |
| Ferry, reflink backend | agents with different edits building at once (wall) | 13.768 s | 15.587 s | 11.336 s | 21.658 s | 3 | –, –, – | ok |
| Ferry base refresh | `ferry base` after a commit, seeded from the previous base | 9.055 s | 9.455 s | 8.959 s | 10.352 s | 3 | 9, 9, 9 | ok |
| Ferry base refresh | `ferry base --cold` after the same commit | 92.500 s | 92.500 s | 87.739 s | 97.261 s | 2 | 280, 279 | ok |

Check: the edited crate's rlib contains exactly the markers of this agent's own edits (none for a build without edits).

## Disk

`du` counts every allocated block, including extents shared with the base; btrfs exclusive counts only blocks no other file shares.

| Scenario | What | du | btrfs exclusive | Ferry private bytes |
|---|---|---:|---:|---:|
| Cold worktree, empty target | target after first edit | 4397.3 MiB | 3727.3 MiB | – |
| kache, warm local store | kache store | 4666.4 MiB | 2613.4 MiB | – |
| Ferry, sandbox off (crate-level reuse only) | base target | 3975.6 MiB | 3965.9 MiB | – |
| Ferry, sandbox off (crate-level reuse only) | agent after spawn | 3975.6 MiB | 0 MiB | – |
| Ferry, sandbox off (crate-level reuse only) | agent after two edits | 4423 MiB | 1072 MiB | 3056.9 MiB |
| Ferry, remap-paths (sandbox off) | base target | 3973.6 MiB | 3914.7 MiB | – |
| Ferry, remap-paths (sandbox off) | agent after spawn | 3973.5 MiB | 0 MiB | – |
| Ferry, remap-paths (sandbox off) | agent after two edits | 4416.6 MiB | 793.7 MiB | 3055.3 MiB |
| Ferry, overlay backend | base target | 3974.5 MiB | 3964.1 MiB | – |
| Ferry, overlay backend | agent after spawn | 0 MiB | 0 MiB | – |
| Ferry, overlay backend | agent after two edits | 1897.6 MiB | 1609.6 MiB | 1897.6 MiB |
| Ferry, reflink backend | base target | 3974.5 MiB | 3965 MiB | – |
| Ferry, reflink backend | agent after spawn | 3974.5 MiB | 0 MiB | – |
| Ferry, reflink backend | agent after two edits | 4374.4 MiB | 949.2 MiB | 2963.6 MiB |

## Concurrent builds

| Scenario | Run | Wall | Per-agent elapsed | Crates per agent | Lock waits per agent (build dir / package cache) | Check |
|---|---:|---:|---|---|---|---|
| Shared `CARGO_TARGET_DIR` | 0 | 8.899 s | 8.658, 8.769, 8.886 s | 10, 1, 1 | 0/4, 1/3, 1/3 | – |
| Shared `CARGO_TARGET_DIR` | 1 | 10.752 s | 10.542, 10.742, 10.641 s | 10, 1, 1 | 0/4, 1/3, 1/3 | – |
| Shared `CARGO_TARGET_DIR` | 2 | 9.815 s | 9.594, 9.803, 9.706 s | 10, 1, 1 | 0/3, 1/4, 1/3 | – |
| Ferry, sandbox off (crate-level reuse only) | 0 | 41.808 s | 41.796, 40.504, 40.501 s | 12, 12, 12 | 0/3, 0/4, 0/3 | ok |
| Ferry, sandbox off (crate-level reuse only) | 1 | 41.326 s | 41.312, 41.153, 41.284 s | 12, 12, 12 | 0/3, 0/4, 0/3 | ok |
| Ferry, sandbox off (crate-level reuse only) | 2 | 69.217 s | 69.205, 69.102, 69.041 s | 12, 12, 12 | 0/4, 0/3, 0/3 | ok |
| Ferry, remap-paths (sandbox off) | 0 | 15.317 s | 15.246, 15.305, 15.181 s | 12, 12, 12 | 0/3, 0/4, 0/3 | ok |
| Ferry, remap-paths (sandbox off) | 1 | 46.966 s | 15.969, 46.956, 46.758 s | 12, 12, 12 | 0/3, 0/4, 0/3 | ok |
| Ferry, remap-paths (sandbox off) | 2 | 16.417 s | 16.381, 16.361, 16.406 s | 12, 12, 12 | 0/3, 0/4, 0/3 | ok |
| Ferry, overlay backend | 0 | 10.661 s | 10.652, 10.652, 10.651 s | 9, 9, 9 | 0/3, 0/3, 0/4 | ok |
| Ferry, overlay backend | 1 | 10.304 s | 10.292, 10.293, 10.294 s | 9, 9, 9 | 0/4, 0/3, 0/3 | ok |
| Ferry, overlay backend | 2 | 10.091 s | 10.081, 10.080, 10.080 s | 9, 9, 9 | 0/4, 0/3, 0/3 | ok |
| Ferry, reflink backend | 0 | 13.768 s | 13.755, 13.757, 13.753 s | 9, 9, 9 | 0/4, 0/3, 0/3 | ok |
| Ferry, reflink backend | 1 | 11.336 s | 11.237, 11.327, 11.236 s | 9, 9, 9 | 0/3, 0/3, 0/4 | ok |
| Ferry, reflink backend | 2 | 21.658 s | 21.646, 21.646, 21.649 s | 9, 9, 9 | 0/4, 0/3, 0/3 | ok |

## Shared-target contamination

Agent B edits, agent A makes a different edit and builds, then B builds (single run).

- A's build compiled 10 crate(s); B's build compiled 1: proc_macro_srv (lib)
- B's rlib contains A's edit: **true**; contains B's own edit: **false**

## kache per-crate results

From kache's `events.jsonl` for each timed build (cargo reports restored crates as compiled, so these are the real compiles).

| Step | Run | Events by result | Compiled (not a cache hit) |
|---|---:|---|---|
| spawn_build | 0 | dup 1, local_hit 48, miss 13, passthrough 3 | build_script_run, build_script_run, test_utils, test_fixture, xtask, load_cargo, ide_ssr, ide_db, ide_diagnostics, ide_completion, ide_assists, ide, rust_analyzer, rust_analyzer |
| spawn_build | 1 | local_hit 62, passthrough 3 |  |
| spawn_build | 2 | local_hit 62, passthrough 3 |  |
| edit_on_spawn | 0 | local_hit 53, miss 9, passthrough 3 | load_cargo, ide_ssr, ide_db, ide_diagnostics, ide_completion, ide_assists, ide, rust_analyzer, rust_analyzer |
| edit_on_spawn | 1 | local_hit 53, miss 9, passthrough 3 | load_cargo, ide_ssr, ide_db, ide_diagnostics, ide_completion, ide_assists, ide, rust_analyzer, rust_analyzer |
| edit_on_spawn | 2 | local_hit 53, miss 9, passthrough 3 | load_cargo, ide_ssr, ide_db, ide_diagnostics, ide_completion, ide_assists, ide, rust_analyzer, rust_analyzer |
| first_edit | 0 | passthrough 9 | load_cargo (adaptive seed), ide_ssr (adaptive seed), ide_db (adaptive seed), ide_diagnostics (adaptive seed), ide_completion (adaptive seed), ide_assists (adaptive seed), ide (adaptive seed), rust_analyzer (adaptive seed), rust_analyzer (adaptive seed) |
| first_edit | 1 | passthrough 9 | load_cargo (adaptive seed), ide_ssr (adaptive seed), ide_db (adaptive seed), ide_diagnostics (adaptive seed), ide_completion (adaptive seed), ide_assists (adaptive seed), ide (adaptive seed), rust_analyzer (adaptive seed), rust_analyzer (adaptive seed) |
| first_edit | 2 | passthrough 9 | load_cargo (adaptive seed), ide_ssr (adaptive seed), ide_db (adaptive seed), ide_diagnostics (adaptive seed), ide_completion (adaptive seed), ide_assists (adaptive seed), ide (adaptive seed), rust_analyzer (adaptive seed), rust_analyzer (adaptive seed) |
| second_edit | 0 | passthrough 9 | ide_ssr (adaptive active), ide_db (adaptive active), load_cargo (adaptive active), ide_diagnostics (adaptive active), ide_completion (adaptive active), ide_assists (adaptive active), ide (adaptive active), rust_analyzer (adaptive active), rust_analyzer (adaptive active) |
| second_edit | 1 | passthrough 9 | ide_ssr (adaptive active), ide_db (adaptive active), load_cargo (adaptive active), ide_diagnostics (adaptive active), ide_completion (adaptive active), ide_assists (adaptive active), ide (adaptive active), rust_analyzer (adaptive active), rust_analyzer (adaptive active) |
| second_edit | 2 | passthrough 9 | ide_ssr (adaptive active), ide_db (adaptive active), load_cargo (adaptive active), ide_diagnostics (adaptive active), ide_completion (adaptive active), ide_assists (adaptive active), ide (adaptive active), rust_analyzer (adaptive active), rust_analyzer (adaptive active) |

kache store warm-up build: 89.177 s, 281 crates (single run, not part of any timed step).

