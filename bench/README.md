# Benchmarks

This harness answers one question: **how long does a newly spawned agent wait
before its first edit compiles?** It compares Ferry with the alternatives an
agent setup would otherwise use, on real workspaces, and records enough
detail (environment, per-run times, crates compiled, artifact checks) for
every number to be checked.

## Running

Prerequisites: `git`, `cargo`/`rustc`, `hyperfine` (1.19+, for `--conclude`),
`jq`, GNU coreutils, `findmnt`; optional `btrfs-progs` for exclusive-bytes
disk numbers; `python3` (standard library only) for the chart. The harness
builds Ferry from this checkout (`cargo build --release`) and installs kache
1.2.2 into `bench/.cache` on first use.

```sh
# Sanity run on the test fixture (seconds).
bench/run.sh --workspace tests/fixtures/workspace --name fixture \
  --edit leaf/src/lib.rs --runs 2

# ripgrep, the prototype's protocol (research/RESULTS.md).
bench/run.sh --workspace https://github.com/BurntSushi/ripgrep --rev 6892bc2 \
  --edit crates/ignore/src/lib.rs --runs 5

# A large workspace.
bench/run.sh --workspace https://github.com/rust-lang/rust-analyzer --rev 0c4d74686b21 \
  --edit crates/ide-db/src/lib.rs --runs 5 --cold-runs 3

# The chart, from any number of results.
python3 bench/chart.py bench/results/ripgrep/results.json bench/results/rust-analyzer/results.json
```

| Option | Meaning |
|---|---|
| `--workspace` | git URL (mirrored once into `bench/.cache/repos`), a git checkout, or a plain directory (copied and committed) |
| `--rev` | commit to benchmark |
| `--edit` | file to edit, relative to the workspace root; it must belong to a library crate |
| `--runs` | timed runs per step (default 5) |
| `--cold-runs` | runs for steps that are, or prepare with, a full cold build (default `--runs`) |
| `--agents` | concurrent agents (default 3) |
| `--scenarios` | comma-separated subset; they always run in the order below |
| `--build-args` | extra arguments for every `cargo build` |
| `--out` | results directory (default `bench/results/<name>`) |

Everything the harness creates lives in `bench/work/<name>/` (gitignored):
the workspace checkout, worktrees, target dirs, `FERRY_HOME`,
`FERRY_WORKTREES`, the kache store, and every build log. Ferry never touches
the user's own cache. Each scenario deletes its target dirs before the next
starts, so at most two large target dirs exist at once (the kache scenario
also keeps the kache store).

Outputs, per run of the harness:

- `results/<name>/env.json`: machine and tool versions (below).
- `results/<name>/results.json`: every step with all run times, stats,
  crates compiled per run and the per-crate list, artifact checks, kache's
  per-crate events, disk usage, concurrency details.
- `results/<name>/results.md`: the same as tables, with the environment header.
- `results/<name>/hyperfine/*.json`: hyperfine's own export for each step.
- `results/chart.svg`: written by `chart.py`.

## What is measured

Every scenario builds with the same command, `cargo build [--build-args]
--message-format=json` (debug profile). **Crates compiled** is the number of
`compiler-artifact` messages with `"fresh": false`; the per-crate list is kept.
This counts units, so a package with a build script counts twice (ripgrep's
cold build is 44 units where `cargo build` prints 34 `Compiling` lines).

The **edit** appends a documented `pub fn` returning a marker string to the
`--edit` file. Every run uses a new function name and marker, so content
caches (kache) can never replay an earlier run's edit, just as they could not
for a real agent's change. After each timed build, a **check** reads the
edited crate's rlib and passes only if it contains exactly the markers of this
agent's own edits (none after a build without edits). This catches stale
builds, not just slow ones.

Steps, each a separate hyperfine benchmark whose `--prepare` creates a new
agent and brings it to the state just before the timed build:

| Step | Agent state before the timed build |
|---|---|
| `spawn_build` | new agent, no edits: the first build after spawn |
| `edit_on_spawn` | new agent plus one edit: **the chart's metric**, "first edit after spawning a new agent" |
| `first_edit` | new agent, one untimed build, then one edit |
| `second_edit` | as `first_edit`, then a second, different edit (steady state) |

`edit_on_spawn` is the chart's metric because it is the build an agent waits
on before its first change compiles. `first_edit` is listed too, but it
flatters the cold and naive scenarios: their untimed first build recompiles
the workspace crates at the new path, which gives them fresh incremental state.

### Scenarios

| Scenario | Agent setup |
|---|---|
| `cold` | `git worktree add` plus an empty target dir. Its `first_edit` is the ideal: incremental at the same path. |
| `naive` | a new worktree (fresh checkout mtimes) plus `cp -a` of a warm target built in another worktree. GNU cp 9+ reflinks where it can, so this is also what `cp --reflink` (Worktrunk's `copy-ignored`) gives. |
| `shared` | N worktrees with one shared `CARGO_TARGET_DIR`: (a) N agents with different edits build at once (wall time, per-agent elapsed, crates, and lock waits split into Cargo's build-directory lock and the package-cache lock); (b) contamination: B edits, A edits differently and builds, then B builds; reports B's crates compiled and whether B's rlib holds A's edit, B's, or both (single run). |
| `kache` | kache 1.2.2 as `RUSTC_WRAPPER` with its daemon running and isolated dirs (`KACHE_CACHE_DIR`, `KACHE_RUNTIME_DIR`, `KACHE_CONFIG`, `KACHE_HOST_CONFIG=`, `XDG_CACHE_HOME`, a short `KACHE_SOCKET_PATH`), as in `research/kache-and-prior-art.md`. The store is warmed by one build in another worktree; each agent is a new worktree with an empty target. Cargo reports restored crates as compiled, so kache's own per-crate events (`events.jsonl`: hits, misses, passthroughs and why) are recorded for every timed build. |
| `ferry-off` | `FERRY_SANDBOX=off`, backend `auto` (reflink on btrfs): crate-level reuse only, no canonical path. |
| `ferry-overlay`, `ferry-reflink`, `ferry-copy` | `FERRY_SANDBOX=namespace` with that `FERRY_BACKEND`. They share one base. |
| `refresh` | commits an edit to the workspace's main checkout, then times `ferry base` (seeded from the previous base) and `ferry base --cold`; checks that the new base's rlib holds the committed edit. |

For every Ferry scenario: `ferry base` (cold, for `ferry-off` and
`ferry-overlay`; the others reuse the overlay base), `ferry spawn`, the four
build steps (builds run as `ferry run <agent> -- cargo build ...`, so Ferry's
sandbox entry is included), disk per agent right after spawn and after two
edits, and N agents spawned, each with a different edit, building at once.
`FERRY_BASE_COMMAND` is set to the measured build command (plus JSON output)
so the base warms exactly what the scenarios build; Ferry's default warm
commands also run `cargo test --no-run`, `cargo check` and `cargo clippy`,
which would make `ferry base` slower than these numbers.

**Disk**: `du` (allocated blocks, counting extents shared with the base) and,
on btrfs, `btrfs filesystem du` exclusive bytes (blocks no other file shares),
plus Ferry's own `private_bytes` estimate.

### Statistics

hyperfine runs each step `--runs` times with no warmup; `--prepare` and
`--conclude` (artifact check, kache events) are not timed. Each prepare ends
with `sync`, so writeback from deleting or cloning a target is not charged to
the timed build. Reported: median,
mean, min, max, standard deviation and every individual time. No run is
dropped; a failing step stops its scenario, is recorded as failed in
`results.json`/`results.md`, and the harness continues with the next one.
A skipped step (only for lack of disk space) is recorded with the reason.
Tables print times to the millisecond; the chart rounds half-up to two
decimals below 10 s for every bar alike. Nothing is extrapolated: every
number is a measured wall time of one command.

### Environment record

`env.json` holds: CPU model, physical cores and logical CPUs, CPU governor,
RAM, available memory and swap in use (start and end), kernel, OS, the bench
directory's filesystem type and mount options, free space, `rustc -vV`,
`cargo -V`, Ferry's version and the git commit it was built from (and whether
`src/` had uncommitted changes), kache, hyperfine, git and cp versions, the
workspace commit, start and end time, and the 1/5/15-minute load averages
before and after. Results taken while the load average is well above zero
were not taken on an idle machine; compare the load numbers with the CPU
count before trusting small differences.

## Workspaces

- **Fixture** (`tests/fixtures/workspace`, edit `leaf/src/lib.rs`): five tiny
  crates. A sanity check of the harness and of Ferry's correctness, not a
  performance result: everything takes well under a second.
- **ripgrep** at `6892bc2`, edit `crates/ignore/src/lib.rs`: the prototype's
  protocol. 44 units cold; the edit rebuilds `ignore` and `rg`.
- **rust-analyzer** at `0c4d74686b21f2b3bd8872b6d3cf36ad2e740d49` (master on
  2026-10-22, the newest commit at the time; it builds with stable 1.99),
  edit `crates/ide-db/src/lib.rs`. 45 workspace members, 281 units cold, a
  4.3 GiB debug target. `ide-db` sits mid-graph: an edit there rebuilds 9
  units (`ide-db`, `ide-assists`, `ide-completion`, `ide-diagnostics`,
  `ide-ssr`, `ide`, `load-cargo`, and the `rust-analyzer` lib and binary),
  the shape of a typical feature change. A lower crate such as `hir-def`
  would also rebuild `hir-ty`, `hir` and everything above them (12 crates,
  a much larger share of the workspace's compile time); a crate near the top
  such as `ide` rebuilds only itself and `rust-analyzer`.

## Caveats

- This is a desktop machine (a btrfs `/home` with zstd compression, other
  software running), not a controlled lab. Read ratios and crate counts
  before absolute seconds, and check the load averages in each results file.
- ripgrep is small; its absolute times are short and Ferry's fixed overhead
  (namespace and overlay setup, a few tens of milliseconds) is a larger share
  of them than on a large workspace.
- On btrfs, `cp -a` (the `naive` scenario) and Ferry's `copy` backend both
  share extents with their source (`cp` reflinks by default; Ferry's copy
  uses `copy_file_range`, which btrfs implements as a clone). Their disk
  numbers here are not those of a byte-for-byte copy, and `ferry spawn` with
  `copy` is far cheaper than on ext4.
- Concurrent builds also contend on Cargo's shared package-cache lock in
  `~/.cargo`, even with separate target dirs; the lock-wait columns show it.
- kache is benchmarked as released (1.2.2). Its unreleased main branch adds
  incremental seeds between worktrees (see `research/kache-and-prior-art.md`).
