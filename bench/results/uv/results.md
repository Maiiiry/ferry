# Ferry benchmark: uv

- Workspace: /home/pat/Projects/ferry/target/probe/uv at `f8bec5eac866c5dd3794b4420e21e5dbd44045c6`
- Edit: append a documented `pub fn` to `crates/uv-resolver/src/lib.rs` (package `uv-resolver`); every run uses a new, unique function
- Build command: `cargo build` (debug), JSON messages; crates = `compiler-artifact` messages with `"fresh": false`
- Runs per step: 2 (steps that are or prepare with a cold build: 1); concurrent agents: 3

## Environment

| | |
|---|---|
| CPU | AMD Ryzen 5 3600X 6-Core Processor (6 cores, 12 logical CPUs, governor powersave) |
| RAM | 31 GiB; available at start 27301.5 MiB, swap in use at start 1211.1 MiB |
| OS / kernel | NixOS 26.11 (Zokor) / 6.18.55 |
| Bench filesystem | btrfs (`rw,noatime,nodiratime,compress=zstd:1,ssd,discard=async,space_cache=v2,subvolid=257,subvol=/@home`) |
| rustc | `rustc 1.99.0 (b940084d7 2026-09-28)` |
| cargo | `cargo 1.99.0 (5f94df478 2026-08-27)` |
| ferry | `ferry 0.1.2` at `9c54ec59f243aba3dc77bcf02e2289a9a696b521` |
| kache | `kache 1.2.2` |
| hyperfine | `hyperfine 1.21.0` |
| Date (UTC) | 2026-10-27T07:18:37Z to 2026-10-27T07:45:05Z |
| Load average (1/5/15 min) | before 12.71 9.31 8.32, after 8.20 7.33 7.50 |

## First edit after spawning a new agent

A new agent is created, one edit is applied, and the timed build is the first build it runs.

| Scenario | Median | Mean | Min | Max | Runs | Crates compiled | Artifact check |
|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | 79.480 s | 79.480 s | 79.480 s | 79.480 s | 1 | 624 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | 44.958 s | 44.958 s | 36.666 s | 53.250 s | 2 | 98, 98 | ok |
| kache, warm local store | 40.570 s | 40.570 s | 40.512 s | 40.628 s | 2 | 113, 113 | ok |
| Ferry, reflink backend | 8.999 s | 8.999 s | 8.560 s | 9.439 s | 2 | 15, 15 | ok |

## All timed steps

| Scenario | Step | Median | Mean | Min | Max | Runs | Crates compiled (per run) | Check |
|---|---|---:|---:|---:|---:|---:|---|---|
| Cold worktree, empty target | first build after spawn, no edit | 80.007 s | 80.007 s | 80.007 s | 80.007 s | 1 | 624 | ok |
| Cold worktree, empty target | **first edit after spawn** (spawn, edit, build) | 79.480 s | 79.480 s | 79.480 s | 79.480 s | 1 | 624 | ok |
| Cold worktree, empty target | first edit after a spawn build (spawn, build, edit, build) | 6.927 s | 6.927 s | 6.927 s | 6.927 s | 1 | 15 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first build after spawn, no edit | 36.817 s | 36.817 s | 36.777 s | 36.857 s | 2 | 98, 98 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | **first edit after spawn** (spawn, edit, build) | 44.958 s | 44.958 s | 36.666 s | 53.250 s | 2 | 98, 98 | ok |
| Naive copy (`cp -a` warm target into a fresh checkout) | first edit after a spawn build (spawn, build, edit, build) | 6.964 s | 6.964 s | 6.929 s | 7.000 s | 2 | 15, 15 | ok |
| kache, warm local store | first build after spawn, no edit | 34.387 s | 34.387 s | 32.794 s | 35.979 s | 2 | 113, 113 | ok |
| kache, warm local store | **first edit after spawn** (spawn, edit, build) | 40.570 s | 40.570 s | 40.512 s | 40.628 s | 2 | 113, 113 | ok |
| kache, warm local store | first edit after a spawn build (spawn, build, edit, build) | 23.119 s | 23.119 s | 18.818 s | 27.420 s | 2 | 15, 15 | ok |
| kache, warm local store | second, different edit (steady state) | 7.087 s | 7.087 s | 7.011 s | 7.163 s | 2 | 15, 15 | ok |
| Ferry, reflink backend | `ferry spawn` | 1.255 s | 1.255 s | 1.248 s | 1.262 s | 2 | –, – | – |
| Ferry, reflink backend | first build after spawn, no edit | 0.462 s | 0.462 s | 0.451 s | 0.474 s | 2 | 0, 0 | ok |
| Ferry, reflink backend | **first edit after spawn** (spawn, edit, build) | 8.999 s | 8.999 s | 8.560 s | 9.439 s | 2 | 15, 15 | ok |
| Ferry, reflink backend | first edit after a spawn build (spawn, build, edit, build) | 8.575 s | 8.575 s | 8.538 s | 8.612 s | 2 | 15, 15 | ok |
| Ferry, reflink backend | second, different edit (steady state) | 6.972 s | 6.972 s | 6.946 s | 6.998 s | 2 | 15, 15 | ok |
| Ferry, reflink backend | agents with different edits building at once (wall) | 12.422 s | 12.422 s | 12.345 s | 12.498 s | 2 | –, – | ok |
| Ferry base refresh | `ferry base` after a commit, seeded from the previous base | 10.009 s | 10.009 s | 9.841 s | 10.176 s | 2 | 15, 15 | ok |
| Ferry base refresh | `ferry base --cold` after the same commit | 79.948 s | 79.948 s | 79.948 s | 79.948 s | 1 | 617 | ok |

Check: the edited crate's rlib contains exactly the markers of this agent's own edits (none for a build without edits).

## Disk

`du` counts every allocated block, including extents shared with the base; btrfs exclusive counts only blocks no other file shares.

| Scenario | What | du | btrfs exclusive | Ferry private bytes |
|---|---|---:|---:|---:|
| Cold worktree, empty target | target after first edit | 5594 MiB | 4680.4 MiB | – |
| kache, warm local store | kache store | 4322.6 MiB | 1292.9 MiB | – |
| Ferry, reflink backend | base target | 5131.3 MiB | 4665.9 MiB | – |
| Ferry, reflink backend | agent after spawn | 5131.3 MiB | 0 MiB | – |
| Ferry, reflink backend | agent after two edits | 5564 MiB | 809.7 MiB | 2678.5 MiB |

## Concurrent builds

| Scenario | Run | Wall | Per-agent elapsed | Crates per agent | Lock waits per agent (build dir / package cache) | Check |
|---|---:|---:|---|---|---|---|
| Ferry, reflink backend | 0 | 12.345 s | 12.336, 11.926, 12.034 s | 15, 15, 15 | 0/4, 0/3, 0/3 | ok |
| Ferry, reflink backend | 1 | 12.498 s | 12.453, 12.489, 12.450 s | 15, 15, 15 | 0/4, 0/3, 0/3 | ok |

## kache per-crate results

From kache's `events.jsonl` for each timed build (cargo reports restored crates as compiled, so these are the real compiles).

| Step | Run | Events by result | Compiled (not a cache hit) |
|---|---:|---|---|
| spawn_build | 0 | dup 1, local_hit 110, miss 4, passthrough 4 | build_script_run (refused|build-script run not recorded: it asks to rerun every time), uv_test, tikv_jemalloc_sys, uvx, uv_dev, uv |
| spawn_build | 1 | local_hit 115, passthrough 4 | build_script_run (refused|build-script run not recorded: it asks to rerun every time) |
| edit_on_spawn | 0 | dup 1, local_hit 100, miss 14, passthrough 4 | uv_resolver, uv_requirements, build_script_run (refused|build-script run not recorded: it asks to rerun every time), uv_dispatch, uv_lock_operations, uv_build_commands, uv_environment_operations, uv_pip_commands, uv_install_operations, uv_workspace_commands, uv_resolve_operations, uv_tool_commands, uv_project_commands, uv, uvx, uv |
| edit_on_spawn | 1 | dup 2, local_hit 100, miss 13, passthrough 4 | uv_resolver, uv_requirements, build_script_run (refused|build-script run not recorded: it asks to rerun every time), uv_dispatch, uv_lock_operations, uv_build_commands, uv_environment_operations, uv_pip_commands, uv_workspace_commands, uv_resolve_operations, uv_install_operations, uv_tool_commands, uv_project_commands, uv, uvx, uv |
| first_edit | 0 | passthrough 15 | uv_resolver (adaptive seed), uv_requirements (adaptive seed), uv_dispatch (adaptive seed), uv_lock_operations (adaptive seed), uv_build_commands (adaptive seed), uv_environment_operations (adaptive seed), uv_install_operations (adaptive seed), uv_pip_commands (adaptive seed), uv_resolve_operations (adaptive seed), uv_workspace_commands (adaptive seed), uv_tool_commands (adaptive seed), uv_project_commands (adaptive seed), uv (adaptive seed), uvx (adaptive seed), uv (adaptive seed) |
| first_edit | 1 | passthrough 15 | uv_resolver (adaptive seed), uv_requirements (adaptive seed), uv_dispatch (adaptive seed), uv_lock_operations (adaptive seed), uv_build_commands (adaptive seed), uv_environment_operations (adaptive seed), uv_pip_commands (adaptive seed), uv_install_operations (adaptive seed), uv_resolve_operations (adaptive seed), uv_workspace_commands (adaptive seed), uv_tool_commands (adaptive seed), uv_project_commands (adaptive seed), uv (adaptive seed), uvx (adaptive seed), uv (adaptive seed) |
| second_edit | 0 | passthrough 15 | uv_resolver (adaptive active), uv_requirements (adaptive active), uv_dispatch (adaptive active), uv_install_operations (adaptive active), uv_resolve_operations (adaptive active), uv_lock_operations (adaptive active), uv_build_commands (adaptive active), uv_environment_operations (adaptive active), uv_pip_commands (adaptive active), uv_workspace_commands (adaptive active), uv_tool_commands (adaptive active), uv_project_commands (adaptive active), uv (adaptive active), uvx (adaptive active), uv (adaptive active) |
| second_edit | 1 | passthrough 15 | uv_requirements (adaptive active), uv_resolver (adaptive active), uv_dispatch (adaptive active), uv_install_operations (adaptive active), uv_resolve_operations (adaptive active), uv_lock_operations (adaptive active), uv_build_commands (adaptive active), uv_environment_operations (adaptive active), uv_pip_commands (adaptive active), uv_workspace_commands (adaptive active), uv_tool_commands (adaptive active), uv_project_commands (adaptive active), uv (adaptive active), uvx (adaptive active), uv (adaptive active) |

kache store warm-up build: 75.915 s, 624 crates (single run, not part of any timed step).

