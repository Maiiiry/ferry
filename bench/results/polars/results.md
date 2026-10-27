# Ferry benchmark: polars

- Workspace: /home/pat/Projects/ferry/target/probe/polars at `d0a98d9c509ddad852337db66261d2d16f1a419e`
- Edit: append a documented `pub fn` to `crates/polars-lazy/src/lib.rs` (package `polars-lazy`); every run uses a new, unique function
- Build command: `cargo build -p polars-sql` (debug), JSON messages; crates = `compiler-artifact` messages with `"fresh": false`
- Runs per step: 2 (steps that are or prepare with a cold build: 1); concurrent agents: 3

## Environment

| | |
|---|---|
| CPU | AMD Ryzen 5 3600X 6-Core Processor (6 cores, 12 logical CPUs, governor powersave) |
| RAM | 31 GiB; available at start 27448.9 MiB, swap in use at start 1174.8 MiB |
| OS / kernel | NixOS 26.11 (Zokor) / 6.18.55 |
| Bench filesystem | btrfs (`rw,noatime,nodiratime,compress=zstd:1,ssd,discard=async,space_cache=v2,subvolid=257,subvol=/@home`) |
| rustc | `rustc 1.100.0-nightly (0dfb098f3 2026-08-31)` |
| cargo | `cargo 1.100.0-nightly (e8cb624d5 2026-08-22)` |
| ferry | `ferry 0.1.2` at `24e0b9f0fa75efd09c3921392f40b32c576bc8d3` |
| kache | `kache 1.2.2` |
| hyperfine | `hyperfine 1.21.0` |
| Date (UTC) | 2026-10-27T05:47:19Z to 2026-10-27T06:04:44Z |
| Load average (1/5/15 min) | before 9.05 10.28 9.31, after 4.10 4.95 6.23 |

## First edit after spawning a new agent

A new agent is created, one edit is applied, and the timed build is the first build it runs.

| Scenario | Median | Mean | Min | Max | Runs | Crates compiled | Artifact check |
|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | 71.013 s | 71.013 s | 71.013 s | 71.013 s | 1 | 212 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | 54.803 s | 54.803 s | 54.657 s | 54.948 s | 2 | 34, 34 | ok |
| kache, warm local store | 9.635 s | 9.635 s | 9.620 s | 9.650 s | 2 | 212, 212 | ok |
| Ferry, reflink backend | 1.998 s | 1.998 s | 1.994 s | 2.003 s | 2 | 2, 2 | ok |

## All timed steps

| Scenario | Step | Median | Mean | Min | Max | Runs | Crates compiled (per run) | Check |
|---|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | first build after spawn, no edit | 71.241 s | 71.241 s | 71.241 s | 71.241 s | 1 | 212 | ok |
| Cold worktree, empty target | **first edit after spawn** (spawn, edit, build) | 71.013 s | 71.013 s | 71.013 s | 71.013 s | 1 | 212 | ok |
| Cold worktree, empty target | first edit after a spawn build (spawn, build, edit, build) | 1.537 s | 1.537 s | 1.537 s | 1.537 s | 1 | 2 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first build after spawn, no edit | 56.604 s | 56.604 s | 55.390 s | 57.818 s | 2 | 34, 34 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | **first edit after spawn** (spawn, edit, build) | 54.803 s | 54.803 s | 54.657 s | 54.948 s | 2 | 34, 34 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first edit after a spawn build (spawn, build, edit, build) | 1.573 s | 1.573 s | 1.528 s | 1.617 s | 2 | 2, 2 | ok |
| kache, warm local store | first build after spawn, no edit | 2.315 s | 2.315 s | 2.305 s | 2.325 s | 2 | 212, 212 | ok |
| kache, warm local store | **first edit after spawn** (spawn, edit, build) | 9.635 s | 9.635 s | 9.620 s | 9.650 s | 2 | 212, 212 | ok |
| kache, warm local store | first edit after a spawn build (spawn, build, edit, build) | 6.431 s | 6.431 s | 6.327 s | 6.536 s | 2 | 2, 2 | ok |
| kache, warm local store | second, different edit (steady state) | 1.621 s | 1.621 s | 1.617 s | 1.625 s | 2 | 2, 2 | ok |
| Ferry, reflink backend | `ferry spawn` | 0.800 s | 0.800 s | 0.795 s | 0.804 s | 2 | –, – | – |
| Ferry, reflink backend | first build after spawn, no edit | 0.261 s | 0.261 s | 0.259 s | 0.262 s | 2 | 0, 0 | ok |
| Ferry, reflink backend | **first edit after spawn** (spawn, edit, build) | 1.998 s | 1.998 s | 1.994 s | 2.003 s | 2 | 2, 2 | ok |
| Ferry, reflink backend | first edit after a spawn build (spawn, build, edit, build) | 2.456 s | 2.456 s | 2.224 s | 2.689 s | 2 | 2, 2 | ok |
| Ferry, reflink backend | second, different edit (steady state) | 1.619 s | 1.619 s | 1.610 s | 1.627 s | 2 | 2, 2 | ok |
| Ferry, reflink backend | agents with different edits building at once (wall) | 2.581 s | 2.581 s | 2.580 s | 2.581 s | 2 | –, – | ok |
| Ferry base refresh | `ferry base` after a commit, seeded from the previous base | 2.890 s | 2.890 s | 2.888 s | 2.892 s | 2 | 2, 2 | ok |
| Ferry base refresh | `ferry base --cold` after the same commit | 70.658 s | 70.658 s | 70.658 s | 70.658 s | 1 | 211 | ok |

Check: the edited crate's rlib contains exactly the markers of this agent's own edits (none for a build without edits).

## Disk

`du` counts every allocated block, including extents shared with the base; btrfs exclusive counts only blocks no other file shares.

| Scenario | What | du | btrfs exclusive | Ferry private bytes |
|---|---|---:|---:|---:|
| Cold worktree, empty target | target after first edit | 4008 MiB | 3916.7 MiB | – |
| kache, warm local store | kache store | 1791.1 MiB | 127.1 MiB | – |
| Ferry, reflink backend | base target | 3922 MiB | 3462.2 MiB | – |
| Ferry, reflink backend | agent after spawn | 3922 MiB | 0 MiB | – |
| Ferry, reflink backend | agent after two edits | 4005.2 MiB | 145.8 MiB | 384.8 MiB |

## Concurrent builds

| Scenario | Run | Wall | Per-agent elapsed | Crates per agent | Lock waits per agent (build dir / package cache) | Check |
|---|---:|---:|---|---|---|---|
| Ferry, reflink backend | 0 | 2.580 s | 2.571, 2.473, 2.521 s | 2, 2, 2 | 0/3, 0/3, 0/4 | ok |
| Ferry, reflink backend | 1 | 2.581 s | 2.533, 2.572, 2.523 s | 2, 2, 2 | 0/3, 0/3, 0/4 | ok |

## kache per-crate results

From kache's `events.jsonl` for each timed build (cargo reports restored crates as compiled, so these are the real compiles).

| Step | Run | Events by result | Compiled (not a cache hit) |
|---|---:|---|---|
| spawn_build | 0 | local_hit 248, passthrough 4 |  |
| spawn_build | 1 | local_hit 248, passthrough 4 |  |
| edit_on_spawn | 0 | local_hit 246, miss 2, passthrough 4 | polars_lazy, polars_sql |
| edit_on_spawn | 1 | local_hit 246, miss 2, passthrough 4 | polars_lazy, polars_sql |
| first_edit | 0 | passthrough 2 | polars_lazy (adaptive seed), polars_sql (adaptive seed) |
| first_edit | 1 | passthrough 2 | polars_lazy (adaptive seed), polars_sql (adaptive seed) |
| second_edit | 0 | passthrough 2 | polars_lazy (adaptive active), polars_sql (adaptive active) |
| second_edit | 1 | passthrough 2 | polars_lazy (adaptive active), polars_sql (adaptive active) |

kache store warm-up build: 62.827 s, 212 crates (single run, not part of any timed step).

