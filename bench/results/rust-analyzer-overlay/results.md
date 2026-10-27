# Ferry benchmark: rust-analyzer

- Workspace: https://github.com/rust-lang/rust-analyzer at `0c4d74686b21f2b3bd8872b6d3cf36ad2e740d49`
- Edit: append a documented `pub fn` to `crates/ide-db/src/lib.rs` (package `ide-db`); every run uses a new, unique function
- Build command: `cargo build` (debug), JSON messages; crates = `compiler-artifact` messages with `"fresh": false`
- Runs per step: 3 (steps that are or prepare with a cold build: 2); concurrent agents: 3

## Environment

| | |
|---|---|
| CPU | AMD Ryzen 5 3600X 6-Core Processor (6 cores, 12 logical CPUs, governor powersave) |
| RAM | 31 GiB; available at start 27248.6 MiB, swap in use at start 1190.3 MiB |
| OS / kernel | NixOS 26.11 (Zokor) / 6.18.55 |
| Bench filesystem | btrfs (`rw,noatime,nodiratime,compress=zstd:1,ssd,discard=async,space_cache=v2,subvolid=257,subvol=/@home`) |
| rustc | `rustc 1.99.0 (b940084d7 2026-09-28)` |
| cargo | `cargo 1.99.0 (5f94df478 2026-08-27)` |
| ferry | `ferry 0.1.2` at `971cc07b42467a47e25bab5db68ec486d38eb675` |
| kache | `kache 1.2.2` |
| hyperfine | `hyperfine 1.21.0` |
| Date (UTC) | 2026-10-27T05:29:34Z to 2026-10-27T05:37:26Z |
| Load average (1/5/15 min) | before 6.68 5.92 6.65, after 21.16 12.10 8.66 |

## First edit after spawning a new agent

A new agent is created, one edit is applied, and the timed build is the first build it runs.

| Scenario | Median | Mean | Min | Max | Runs | Crates compiled | Artifact check |
|---|---:|---:|---:|---:|---:|---|---|
| Ferry, overlay backend | 7.309 s | 7.477 s | 7.276 s | 7.847 s | 3 | 9, 9, 9 | ok |

## All timed steps

| Scenario | Step | Median | Mean | Min | Max | Runs | Crates compiled (per run) | Check |
|---|---|---:|---:|---:|---:|---:|---|---|
| Ferry, overlay backend | `ferry base` (cold) | 60.136 s | 60.136 s | 59.555 s | 60.716 s | 2 | 280, 281 | – |
| Ferry, overlay backend | `ferry spawn` | 0.444 s | 0.441 s | 0.434 s | 0.444 s | 3 | –, –, – | – |
| Ferry, overlay backend | first build after spawn, no edit | 0.242 s | 0.242 s | 0.239 s | 0.245 s | 3 | 0, 0, 0 | ok |
| Ferry, overlay backend | **first edit after spawn** (spawn, edit, build) | 7.309 s | 7.477 s | 7.276 s | 7.847 s | 3 | 9, 9, 9 | ok |
| Ferry, overlay backend | first edit after a spawn build (spawn, build, edit, build) | 7.787 s | 7.783 s | 7.151 s | 8.412 s | 3 | 9, 9, 9 | ok |
| Ferry, overlay backend | second, different edit (steady state) | 7.295 s | 20.715 s | 7.220 s | 47.630 s | 3 | 9, 9, 9 | ok |
| Ferry, overlay backend | agents with different edits building at once (wall) | 45.774 s | 46.081 s | 9.905 s | 82.564 s | 3 | –, –, – | ok |

Check: the edited crate's rlib contains exactly the markers of this agent's own edits (none for a build without edits).

## Disk

`du` counts every allocated block, including extents shared with the base; btrfs exclusive counts only blocks no other file shares.

| Scenario | What | du | btrfs exclusive | Ferry private bytes |
|---|---|---:|---:|---:|
| Ferry, overlay backend | base target | 3974.5 MiB | 3505.9 MiB | – |
| Ferry, overlay backend | agent after spawn | 0 MiB | 0 MiB | – |
| Ferry, overlay backend | agent after two edits | 1897.6 MiB | 1609.6 MiB | 1897.6 MiB |

## Concurrent builds

| Scenario | Run | Wall | Per-agent elapsed | Crates per agent | Lock waits per agent (build dir / package cache) | Check |
|---|---:|---:|---|---|---|---|
| Ferry, overlay backend | 0 | 9.905 s | 9.895, 9.895, 9.883 s | 9, 9, 9 | 0/4, 0/3, 0/3 | ok |
| Ferry, overlay backend | 1 | 45.774 s | 45.764, 25.151, 45.620 s | 9, 9, 9 | 0/3, 0/3, 0/4 | ok |
| Ferry, overlay backend | 2 | 82.564 s | 82.555, 82.555, 47.382 s | 9, 9, 9 | 0/4, 0/3, 0/3 | ok |

