# Ferry benchmark: ripgrep

- Workspace: https://github.com/BurntSushi/ripgrep at `6892bc2b7efe5909cbbeec8f757ed2c4c76b1d30`
- Edit: append a documented `pub fn` to `crates/ignore/src/lib.rs` (package `ignore`); every run uses a new, unique function
- Build command: `cargo build` (debug), JSON messages; crates = `compiler-artifact` messages with `"fresh": false`
- Runs per step: 3 (steps that are or prepare with a cold build: 2); concurrent agents: 3

## Environment

| | |
|---|---|
| CPU | AMD Ryzen 5 3600X 6-Core Processor (6 cores, 12 logical CPUs, governor powersave) |
| RAM | 31 GiB; available at start 21659.9 MiB, swap in use at start 2784.5 MiB |
| OS / kernel | NixOS 26.11 (Zokor) / 6.18.55 |
| Bench filesystem | btrfs (`rw,noatime,nodiratime,compress=zstd:1,ssd,discard=async,space_cache=v2,subvolid=257,subvol=/@home`) |
| rustc | `rustc 1.99.0 (b940084d7 2026-09-28)` |
| cargo | `cargo 1.99.0 (5f94df478 2026-08-27)` |
| ferry | `ferry 0.1.0` at `d7fb5cb8abde0cb5d0b527a36c341c64fe83bf9f` |
| kache | `kache 1.2.2` |
| hyperfine | `hyperfine 1.21.0` |
| Date (UTC) | 2026-10-24T20:53:10Z to 2026-10-24T21:01:22Z |
| Load average (1/5/15 min) | before 1.21 3.40 3.67, after 4.86 5.20 4.55 |

## First edit after spawning a new agent

A new agent is created, one edit is applied, and the timed build is the first build it runs.

| Scenario | Median | Mean | Min | Max | Runs | Crates compiled | Artifact check |
|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | 7.063 s | 7.063 s | 7.019 s | 7.107 s | 2 | 44, 44 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | 3.675 s | 3.653 s | 3.598 s | 3.687 s | 3 | 10, 10, 10 | ok |
| kache, warm local store | 2.863 s | 2.858 s | 2.836 s | 2.876 s | 3 | 10, 10, 10 | ok |
| Ferry, sandbox off (crate-level reuse only) | 3.204 s | 3.264 s | 3.107 s | 3.480 s | 3 | 2, 2, 2 | ok |
| Ferry, remap-paths (sandbox off) | 2.000 s | 2.003 s | 1.975 s | 2.033 s | 3 | 2, 2, 2 | ok |
| Ferry, overlay backend | 1.252 s | 1.259 s | 1.251 s | 1.274 s | 3 | 2, 2, 2 | ok |
| Ferry, reflink backend | 1.130 s | 1.131 s | 1.126 s | 1.137 s | 3 | 2, 2, 2 | ok |

## All timed steps

| Scenario | Step | Median | Mean | Min | Max | Runs | Crates compiled (per run) | Check |
|---|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | first build after spawn, no edit | 7.400 s | 7.400 s | 7.250 s | 7.550 s | 2 | 44, 44 | ok |
| Cold worktree, empty target | **first edit after spawn** (spawn, edit, build) | 7.063 s | 7.063 s | 7.019 s | 7.107 s | 2 | 44, 44 | ok |
| Cold worktree, empty target | first edit after a spawn build (spawn, build, edit, build) | 0.885 s | 0.885 s | 0.876 s | 0.893 s | 2 | 2, 2 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first build after spawn, no edit | 3.749 s | 3.719 s | 3.533 s | 3.876 s | 3 | 10, 10, 10 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | **first edit after spawn** (spawn, edit, build) | 3.675 s | 3.653 s | 3.598 s | 3.687 s | 3 | 10, 10, 10 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first edit after a spawn build (spawn, build, edit, build) | 0.866 s | 0.871 s | 0.853 s | 0.894 s | 3 | 2, 2, 2 | ok |
| Shared `CARGO_TARGET_DIR` | agents with different edits building at once (wall) | 6.663 s | 6.716 s | 6.233 s | 7.251 s | 3 | –, –, – | – |
| kache, warm local store | first build after spawn, no edit | 0.267 s | 0.262 s | 0.243 s | 0.276 s | 3 | 10, 10, 10 | ok |
| kache, warm local store | **first edit after spawn** (spawn, edit, build) | 2.863 s | 2.858 s | 2.836 s | 2.876 s | 3 | 10, 10, 10 | ok |
| kache, warm local store | first edit after a spawn build (spawn, build, edit, build) | 3.291 s | 3.293 s | 3.265 s | 3.324 s | 3 | 2, 2, 2 | ok |
| kache, warm local store | second, different edit (steady state) | 0.912 s | 1.479 s | 0.902 s | 2.624 s | 3 | 2, 2, 2 | ok |
| Ferry, sandbox off (crate-level reuse only) | `ferry base` (cold) | 13.528 s | 13.528 s | 9.786 s | 17.270 s | 2 | 42, 41 | – |
| Ferry, sandbox off (crate-level reuse only) | `ferry spawn` | 0.169 s | 0.167 s | 0.160 s | 0.171 s | 3 | –, –, – | – |
| Ferry, sandbox off (crate-level reuse only) | first build after spawn, no edit | 0.062 s | 0.063 s | 0.062 s | 0.065 s | 3 | 0, 0, 0 | ok |
| Ferry, sandbox off (crate-level reuse only) | **first edit after spawn** (spawn, edit, build) | 3.204 s | 3.264 s | 3.107 s | 3.480 s | 3 | 2, 2, 2 | ok |
| Ferry, sandbox off (crate-level reuse only) | first edit after a spawn build (spawn, build, edit, build) | 3.324 s | 3.305 s | 3.156 s | 3.433 s | 3 | 2, 2, 2 | ok |
| Ferry, sandbox off (crate-level reuse only) | second, different edit (steady state) | 0.940 s | 0.940 s | 0.925 s | 0.956 s | 3 | 2, 2, 2 | ok |
| Ferry, sandbox off (crate-level reuse only) | agents with different edits building at once (wall) | 4.130 s | 4.126 s | 4.087 s | 4.160 s | 3 | –, –, – | ok |
| Ferry, remap-paths (sandbox off) | `ferry base` (cold) | 6.926 s | 6.926 s | 6.924 s | 6.927 s | 2 | 44, 44 | – |
| Ferry, remap-paths (sandbox off) | `ferry spawn` | 0.168 s | 0.165 s | 0.154 s | 0.174 s | 3 | –, –, – | – |
| Ferry, remap-paths (sandbox off) | first build after spawn, no edit | 0.060 s | 0.064 s | 0.060 s | 0.073 s | 3 | 0, 0, 0 | ok |
| Ferry, remap-paths (sandbox off) | **first edit after spawn** (spawn, edit, build) | 2.000 s | 2.003 s | 1.975 s | 2.033 s | 3 | 2, 2, 2 | ok |
| Ferry, remap-paths (sandbox off) | first edit after a spawn build (spawn, build, edit, build) | 2.044 s | 2.043 s | 2.008 s | 2.077 s | 3 | 2, 2, 2 | ok |
| Ferry, remap-paths (sandbox off) | second, different edit (steady state) | 0.890 s | 0.898 s | 0.881 s | 0.923 s | 3 | 2, 2, 2 | ok |
| Ferry, remap-paths (sandbox off) | agents with different edits building at once (wall) | 2.753 s | 2.746 s | 2.697 s | 2.788 s | 3 | –, –, – | ok |
| Ferry, overlay backend | `ferry base` (cold) | 7.552 s | 7.552 s | 7.472 s | 7.633 s | 2 | 44, 44 | – |
| Ferry, overlay backend | `ferry spawn` | 0.103 s | 0.105 s | 0.103 s | 0.107 s | 3 | –, –, – | – |
| Ferry, overlay backend | first build after spawn, no edit | 0.084 s | 0.085 s | 0.084 s | 0.086 s | 3 | 0, 0, 0 | ok |
| Ferry, overlay backend | **first edit after spawn** (spawn, edit, build) | 1.252 s | 1.259 s | 1.251 s | 1.274 s | 3 | 2, 2, 2 | ok |
| Ferry, overlay backend | first edit after a spawn build (spawn, build, edit, build) | 1.269 s | 1.283 s | 1.234 s | 1.345 s | 3 | 2, 2, 2 | ok |
| Ferry, overlay backend | second, different edit (steady state) | 1.066 s | 1.069 s | 1.055 s | 1.085 s | 3 | 2, 2, 2 | ok |
| Ferry, overlay backend | agents with different edits building at once (wall) | 1.572 s | 1.569 s | 1.558 s | 1.575 s | 3 | –, –, – | ok |
| Ferry, reflink backend | `ferry spawn` | 0.156 s | 0.156 s | 0.148 s | 0.164 s | 3 | –, –, – | – |
| Ferry, reflink backend | first build after spawn, no edit | 0.060 s | 0.060 s | 0.058 s | 0.061 s | 3 | 0, 0, 0 | ok |
| Ferry, reflink backend | **first edit after spawn** (spawn, edit, build) | 1.130 s | 1.131 s | 1.126 s | 1.137 s | 3 | 2, 2, 2 | ok |
| Ferry, reflink backend | first edit after a spawn build (spawn, build, edit, build) | 1.137 s | 1.134 s | 1.114 s | 1.151 s | 3 | 2, 2, 2 | ok |
| Ferry, reflink backend | second, different edit (steady state) | 0.878 s | 0.888 s | 0.878 s | 0.907 s | 3 | 2, 2, 2 | ok |
| Ferry, reflink backend | agents with different edits building at once (wall) | 1.409 s | 1.409 s | 1.393 s | 1.426 s | 3 | –, –, – | ok |
| Ferry base refresh | `ferry base` after a commit, seeded from the previous base | 1.338 s | 1.337 s | 1.323 s | 1.349 s | 3 | 2, 2, 2 | ok |
| Ferry base refresh | `ferry base --cold` after the same commit | 7.414 s | 7.414 s | 7.090 s | 7.738 s | 2 | 44, 44 | ok |

Check: the edited crate's rlib contains exactly the markers of this agent's own edits (none for a build without edits).

## Disk

`du` counts every allocated block, including extents shared with the base; btrfs exclusive counts only blocks no other file shares.

| Scenario | What | du | btrfs exclusive | Ferry private bytes |
|---|---|---:|---:|---:|
| Cold worktree, empty target | target after first edit | 402.7 MiB | 324.4 MiB | – |
| kache, warm local store | kache store | 416.2 MiB | 184.8 MiB | – |
| Ferry, sandbox off (crate-level reuse only) | base target | 366 MiB | 363.5 MiB | – |
| Ferry, sandbox off (crate-level reuse only) | agent after spawn | 366 MiB | 0 MiB | – |
| Ferry, sandbox off (crate-level reuse only) | agent after two edits | 402.9 MiB | 111 MiB | 284.4 MiB |
| Ferry, remap-paths (sandbox off) | base target | 365.7 MiB | 282.1 MiB | – |
| Ferry, remap-paths (sandbox off) | agent after spawn | 365.7 MiB | 0 MiB | – |
| Ferry, remap-paths (sandbox off) | agent after two edits | 401.8 MiB | 94.2 MiB | 283.5 MiB |
| Ferry, overlay backend | base target | 365.9 MiB | 237.3 MiB | – |
| Ferry, overlay backend | agent after spawn | 0 MiB | 0 MiB | – |
| Ferry, overlay backend | agent after two edits | 186.2 MiB | 141.5 MiB | 186.2 MiB |
| Ferry, reflink backend | base target | 365.9 MiB | 364.5 MiB | – |
| Ferry, reflink backend | agent after spawn | 365.9 MiB | 0 MiB | – |
| Ferry, reflink backend | agent after two edits | 400.9 MiB | 64.6 MiB | 282.6 MiB |

## Concurrent builds

| Scenario | Run | Wall | Per-agent elapsed | Crates per agent | Lock waits per agent (build dir / package cache) | Check |
|---|---:|---:|---|---|---|---|
| Shared `CARGO_TARGET_DIR` | 0 | 6.233 s | 6.223, 4.285, 2.382 s | 1, 1, 2 | 1/3, 1/3, 0/4 | – |
| Shared `CARGO_TARGET_DIR` | 1 | 6.663 s | 2.532, 4.567, 6.653 s | 2, 1, 1 | 0/4, 1/3, 1/3 | – |
| Shared `CARGO_TARGET_DIR` | 2 | 7.251 s | 7.239, 3.416, 5.314 s | 1, 2, 1 | 1/4, 0/2, 1/3 | – |
| Ferry, sandbox off (crate-level reuse only) | 0 | 4.160 s | 4.106, 4.106, 4.149 s | 2, 2, 2 | 0/4, 0/3, 0/3 | ok |
| Ferry, sandbox off (crate-level reuse only) | 1 | 4.087 s | 3.965, 4.077, 4.012 s | 2, 2, 2 | 0/3, 0/3, 0/4 | ok |
| Ferry, sandbox off (crate-level reuse only) | 2 | 4.130 s | 4.119, 4.106, 4.106 s | 2, 2, 2 | 0/3, 0/4, 0/3 | ok |
| Ferry, remap-paths (sandbox off) | 0 | 2.697 s | 2.659, 2.688, 2.677 s | 2, 2, 2 | 0/3, 0/4, 0/3 | ok |
| Ferry, remap-paths (sandbox off) | 1 | 2.753 s | 2.739, 2.739, 2.743 s | 2, 2, 2 | 0/3, 0/4, 0/3 | ok |
| Ferry, remap-paths (sandbox off) | 2 | 2.788 s | 2.770, 2.767, 2.776 s | 2, 2, 2 | 0/3, 0/4, 0/3 | ok |
| Ferry, overlay backend | 0 | 1.558 s | 1.548, 1.548, 1.547 s | 2, 2, 2 | 0/3, 0/3, 0/4 | ok |
| Ferry, overlay backend | 1 | 1.575 s | 1.566, 1.565, 1.565 s | 2, 2, 2 | 0/3, 0/4, 0/3 | ok |
| Ferry, overlay backend | 2 | 1.572 s | 1.562, 1.562, 1.562 s | 2, 2, 2 | 0/3, 0/4, 0/3 | ok |
| Ferry, reflink backend | 0 | 1.426 s | 1.380, 1.408, 1.416 s | 2, 2, 2 | 0/4, 0/3, 0/3 | ok |
| Ferry, reflink backend | 1 | 1.409 s | 1.320, 1.398, 1.337 s | 2, 2, 2 | 0/4, 0/3, 0/3 | ok |
| Ferry, reflink backend | 2 | 1.393 s | 1.353, 1.383, 1.299 s | 2, 2, 2 | 0/4, 0/3, 0/3 | ok |

## Shared-target contamination

Agent B edits, agent A makes a different edit and builds, then B builds (single run).

- A's build compiled 2 crate(s); B's build compiled 1: rg (bin)
- B's rlib contains A's edit: **true**; contains B's own edit: **false**

## kache per-crate results

From kache's `events.jsonl` for each timed build (cargo reports restored crates as compiled, so these are the real compiles).

| Step | Run | Events by result | Compiled (not a cache hit) |
|---|---:|---|---|
| spawn_build | 0 | local_hit 11, passthrough 3 |  |
| spawn_build | 1 | local_hit 11, passthrough 3 |  |
| spawn_build | 2 | local_hit 11, passthrough 3 |  |
| edit_on_spawn | 0 | local_hit 8, miss 3, passthrough 3 | build_script_run, ignore, rg |
| edit_on_spawn | 1 | local_hit 8, miss 3, passthrough 3 | build_script_run, ignore, rg |
| edit_on_spawn | 2 | local_hit 8, miss 3, passthrough 3 | build_script_run, ignore, rg |
| first_edit | 0 | passthrough 2 | ignore (adaptive seed), rg (adaptive seed) |
| first_edit | 1 | passthrough 2 | ignore (adaptive seed), rg (adaptive seed) |
| first_edit | 2 | passthrough 2 | ignore (adaptive seed), rg (adaptive seed) |
| second_edit | 0 | passthrough 2 | ignore (adaptive active), rg (adaptive active) |
| second_edit | 1 | passthrough 2 | ignore (adaptive active), rg (adaptive active) |
| second_edit | 2 | passthrough 2 | ignore (adaptive active), rg (adaptive active) |

kache store warm-up build: 6.418 s, 44 crates (single run, not part of any timed step).

