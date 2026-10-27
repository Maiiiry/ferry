# Ferry benchmark: ruff

- Workspace: /home/pat/Projects/ferry/target/probe/ruff at `8aeb8e2ebc09714ba6c77d9f48c84536f7814e3d`
- Edit: append a documented `pub fn` to `crates/ruff_linter/src/lib.rs` (package `ruff_linter`); every run uses a new, unique function
- Build command: `cargo build` (debug), JSON messages; crates = `compiler-artifact` messages with `"fresh": false`
- Runs per step: 2 (steps that are or prepare with a cold build: 1); concurrent agents: 3

## Environment

| | |
|---|---|
| CPU | AMD Ryzen 5 3600X 6-Core Processor (6 cores, 12 logical CPUs, governor powersave) |
| RAM | 31 GiB; available at start 27488.2 MiB, swap in use at start 1206 MiB |
| OS / kernel | NixOS 26.11 (Zokor) / 6.18.55 |
| Bench filesystem | btrfs (`rw,noatime,nodiratime,compress=zstd:1,ssd,discard=async,space_cache=v2,subvolid=257,subvol=/@home`) |
| rustc | `rustc 1.99.0 (b940084d7 2026-09-28)` |
| cargo | `cargo 1.99.0 (5f94df478 2026-08-27)` |
| ferry | `ferry 0.1.2` at `9c54ec59f243aba3dc77bcf02e2289a9a696b521` |
| kache | `kache 1.2.2` |
| hyperfine | `hyperfine 1.21.0` |
| Date (UTC) | 2026-10-27T06:38:44Z to 2026-10-27T07:18:36Z |
| Load average (1/5/15 min) | before 0.52 1.47 2.20, after 12.71 9.31 8.32 |

## First edit after spawning a new agent

A new agent is created, one edit is applied, and the timed build is the first build it runs.

| Scenario | Median | Mean | Min | Max | Runs | Crates compiled | Artifact check |
|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | 142.070 s | 142.070 s | 142.070 s | 142.070 s | 1 | 505 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | 106.010 s | 106.010 s | 89.726 s | 122.294 s | 2 | 68, 68 | ok |
| kache, warm local store | 40.347 s | 40.347 s | 40.017 s | 40.677 s | 2 | 70, 70 | ok |
| Ferry, reflink backend | 9.972 s | 9.972 s | 9.971 s | 9.974 s | 2 | 14, 14 | ok |

## All timed steps

| Scenario | Step | Median | Mean | Min | Max | Runs | Crates compiled (per run) | Check |
|---|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | first build after spawn, no edit | 131.606 s | 131.606 s | 131.606 s | 131.606 s | 1 | 505 | ok |
| Cold worktree, empty target | **first edit after spawn** (spawn, edit, build) | 142.070 s | 142.070 s | 142.070 s | 142.070 s | 1 | 505 | ok |
| Cold worktree, empty target | first edit after a spawn build (spawn, build, edit, build) | 9.056 s | 9.056 s | 9.056 s | 9.056 s | 1 | 11 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first build after spawn, no edit | 89.727 s | 89.727 s | 89.384 s | 90.070 s | 2 | 68, 68 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | **first edit after spawn** (spawn, edit, build) | 106.010 s | 106.010 s | 89.726 s | 122.294 s | 2 | 68, 68 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first edit after a spawn build (spawn, build, edit, build) | 29.226 s | 29.226 s | 9.064 s | 49.387 s | 2 | 11, 11 | ok |
| kache, warm local store | first build after spawn, no edit | 24.647 s | 24.647 s | 20.255 s | 29.040 s | 2 | 70, 70 | ok |
| kache, warm local store | **first edit after spawn** (spawn, edit, build) | 40.347 s | 40.347 s | 40.017 s | 40.677 s | 2 | 70, 70 | ok |
| kache, warm local store | first edit after a spawn build (spawn, build, edit, build) | 34.796 s | 34.796 s | 33.914 s | 35.679 s | 2 | 11, 11 | ok |
| kache, warm local store | second, different edit (steady state) | 30.966 s | 30.966 s | 10.399 s | 51.534 s | 2 | 11, 11 | ok |
| Ferry, reflink backend | `ferry spawn` | 2.213 s | 2.213 s | 2.123 s | 2.302 s | 2 | –, – | – |
| Ferry, reflink backend | first build after spawn, no edit | 3.239 s | 3.239 s | 3.222 s | 3.256 s | 2 | 6, 6 | ok |
| Ferry, reflink backend | **first edit after spawn** (spawn, edit, build) | 9.972 s | 9.972 s | 9.971 s | 9.974 s | 2 | 14, 14 | ok |
| Ferry, reflink backend | first edit after a spawn build (spawn, build, edit, build) | 9.258 s | 9.258 s | 9.247 s | 9.270 s | 2 | 11, 11 | ok |
| Ferry, reflink backend | second, different edit (steady state) | 9.017 s | 9.017 s | 8.943 s | 9.091 s | 2 | 11, 11 | ok |
| Ferry, reflink backend | agents with different edits building at once (wall) | 16.447 s | 16.447 s | 15.030 s | 17.864 s | 2 | –, – | ok |
| Ferry base refresh | `ferry base` after a commit, seeded from the previous base | 13.479 s | 13.479 s | 12.561 s | 14.397 s | 2 | 14, 14 | ok |
| Ferry base refresh | `ferry base --cold` after the same commit | 162.943 s | 162.943 s | 162.943 s | 162.943 s | 1 | 500 | ok |

Check: the edited crate's rlib contains exactly the markers of this agent's own edits (none for a build without edits).

## Disk

`du` counts every allocated block, including extents shared with the base; btrfs exclusive counts only blocks no other file shares.

| Scenario | What | du | btrfs exclusive | Ferry private bytes |
|---|---|---:|---:|---:|
| Cold worktree, empty target | target after first edit | 6715.9 MiB | 5691.3 MiB | – |
| kache, warm local store | kache store | 5119.9 MiB | 1149.8 MiB | – |
| Ferry, reflink backend | base target | 6204.4 MiB | 5910.1 MiB | – |
| Ferry, reflink backend | agent after spawn | 6204.4 MiB | 0 MiB | – |
| Ferry, reflink backend | agent after two edits | 6747.3 MiB | 1341 MiB | 3786.9 MiB |

## Concurrent builds

| Scenario | Run | Wall | Per-agent elapsed | Crates per agent | Lock waits per agent (build dir / package cache) | Check |
|---|---:|---:|---|---|---|---|
| Ferry, reflink backend | 0 | 17.864 s | 17.855, 17.804, 17.804 s | 14, 14, 14 | 0/3, 0/3, 0/4 | ok |
| Ferry, reflink backend | 1 | 15.030 s | 15.009, 15.021, 15.017 s | 14, 14, 14 | 0/4, 0/3, 0/3 | ok |

## kache per-crate results

From kache's `events.jsonl` for each timed build (cargo reports restored crates as compiled, so these are the real compiles).

| Step | Run | Events by result | Compiled (not a cache hit) |
|---|---:|---|---|
| spawn_build | 0 | dup 3, local_hit 62, miss 10, passthrough 3 | build_script_run, build_script_run, build_script_run, mdtest, ruff_mdtest, ty_test, ty_wasm, tikv_jemalloc_sys, ty, ty, ruff, ruff, ruff_dev |
| spawn_build | 1 | local_hit 75, passthrough 3 |  |
| edit_on_spawn | 0 | dup 3, local_hit 64, miss 8, passthrough 3 | ruff_graph, ruff_benchmark, ruff_markdown, ruff_mdtest, ruff_linter, ruff_workspace, ruff_server, ruff, ruff, ruff_wasm, ruff_dev |
| edit_on_spawn | 1 | dup 3, local_hit 64, miss 8, passthrough 3 | ruff_graph, ruff_benchmark, ruff_markdown, ruff_linter, ruff_mdtest, ruff_workspace, ruff_server, ruff, ruff, ruff_wasm, ruff_dev |
| first_edit | 0 | miss 1, passthrough 10 | ruff_benchmark (adaptive seed), ruff_graph (adaptive seed), ruff_markdown (adaptive seed), ruff_mdtest (adaptive seed), ruff_workspace (adaptive seed), ruff_linter (adaptive seed), ruff_server (adaptive seed), ruff (adaptive seed), ruff (adaptive seed), ruff_dev (adaptive seed), ruff_wasm |
| first_edit | 1 | miss 1, passthrough 10 | ruff_benchmark (adaptive seed), ruff_graph (adaptive seed), ruff_markdown (adaptive seed), ruff_mdtest (adaptive seed), ruff_workspace (adaptive seed), ruff_linter (adaptive seed), ruff_server (adaptive seed), ruff (adaptive seed), ruff (adaptive seed), ruff_dev (adaptive seed), ruff_wasm |
| second_edit | 0 | miss 1, passthrough 10, unknown 1 | ruff_graph (adaptive active), ruff_benchmark (adaptive active), ruff_linter, ruff_markdown (adaptive active), ruff_workspace (adaptive active), ruff_mdtest (adaptive active), ruff_linter (adaptive active), ruff_server (adaptive active), ruff (adaptive active), ruff (adaptive active), ruff_dev (adaptive active), ruff_wasm |
| second_edit | 1 | miss 1, passthrough 10 | ruff_benchmark (adaptive active), ruff_graph (adaptive active), ruff_linter (adaptive active), ruff_markdown (adaptive active), ruff_workspace (adaptive active), ruff_mdtest (adaptive active), ruff_server (adaptive active), ruff (adaptive active), ruff (adaptive active), ruff_dev (adaptive active), ruff_wasm |

kache store warm-up build: 127.940 s, 505 crates (single run, not part of any timed step).

