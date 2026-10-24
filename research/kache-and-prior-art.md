# kache, sccache and prior art

Research brief for Ferry's comparison section and benchmark harness. Date: 2026-10-23.

Each claim is tagged. **V** means verified from a primary source (link given). **M** means measured
locally (command and numbers given). **I** means inferred, not verified.

Local box: NixOS, kernel 6.18, x86_64, 12 cores, tmpfs `/tmp`, cargo/rustc 1.99.0. Experiments ran in
`$S = <scratchpad>/research-kache`. The scripts are `rg-bench.sh` (ripgrep scenarios), `ferry-shim.sh`,
`relwrap.sh` and `tw.sh` (per-crate timing wrapper).

## TL;DR

1. **kache is the closest competitor, and it is about to narrow Ferry's gap.**
   - The released kache 1.2.2 strips `-C incremental`, so a new worktree's first edit is not
     incremental. **M:** 3.1 s on ripgrep, versus 0.81-0.93 s for a Ferry-style fork.
   - kache **main** (unreleased; commits on 2026-10-19 and 2026-10-21) adds automatic
     *incremental seeds between worktrees*. It uses `-Zremap-cwd-prefix=/src`, a child-only
     `RUSTC_BOOTSTRAP=1`, and cwd-relative path operands.
   - **M:** first edit falls to 1.9 s, but only after some other worktree has already edited
     that crate. The bin crate was not seeded.
2. **sccache 0.18.0 is not a worktree tool for workspace crates.**
   - **V:** the Rust cache key hashes the cwd and every `CARGO_*` env var, including the absolute
     `CARGO_MANIFEST_DIR`. `SCCACHE_BASEDIRS` is C/C++-only for now; four Rust PRs are open.
   - **V:** sccache refuses `-C incremental` units, and exits if `CARGO_INCREMENTAL=1` is set.
   - **M:** a new worktree spawn costs 4.7-5.3 s on ripgrep. Only registry crates hit.
3. **Cargo-native cross-workspace caching is a 2026 project goal and not usable yet.**
   - **V:** Cargo 1.100 (2026-11-12) changes the build-dir layout.
   - **V/M:** two Cargo processes on one target dir fully serialize on
     `.cargo-build-lock` ("Blocking waiting for file lock on build directory").
4. **The agent worktree managers do nothing Rust-specific, with one exception.** Worktrunk's
   `wt step copy-ignored` reflinks `target/`. It deliberately keeps source mtimes, so workspace
   crates rebuild. Nobody else does canonical-path incremental reuse.
5. **Composition gotcha.**
   - **V:** kache strips Cargo's `-C incremental` even for passthrough and bypassed crates.
   - **M:** so a Ferry base built through kache has no incremental state.
   - **M:** a 5-line dispatch shim fixes it: workspace members go straight to rustc, deps go to
     kache. Measured base build 3.6 s (cold is 7.7 s) and first edit in the fork 0.85 s.
   - **V/M:** `RUSTC_WRAPPER` does not change Cargo fingerprints; `RUSTC_WORKSPACE_WRAPPER` does.

## 1. Local benchmark: ripgrep @ 6892bc2

The prototype's workload: 34 crates compiled, debug `cargo build`. The edit appends a documented
`pub fn` to `crates/ignore/src/lib.rs`, which forces `ignore` and `rg` to rebuild. Wall times are
single shots and "n" is the number of independent worktrees. Command: `bash $S/rg-bench.sh <scenario>`.

| Scenario (M) | Spawn build | First edit | 2nd edit | Notes |
|---|---|---|---|---|
| Plain cargo, cold worktree | 6.71 s (34 crates) | 0.76 s (same path) | 0.75 s | Same-path edit is the ideal Ferry targets |
| `cp -a` warm target, both trees stamped, different path (n=3) | 0.05 s (0) | 2.99-3.18 s | n/a | Prototype "copied+stamped" row |
| Same, plus `RUSTFLAGS=-Zremap-cwd-prefix=/src RUSTC_BOOTSTRAP=1` (n=3) | 0.04-0.05 s | 2.24-2.36 s | 0.75-0.82 s | The flag alone only partly helps |
| Same, plus `relwrap.sh` (cwd-relative `--out-dir`/`-L`/`--extern`) (n=2) | n/a | 1.73-1.78 s | n/a | `ignore` 208 ms (same path 220 ms); `rg` 1444 ms (same path 523 ms) |
| kache 1.2.2, warm local store, daemon on (n=3) | 0.26-0.31 s | 3.09-3.18 s | 0.73-0.78 s | Registry units seeded; 9 workspace units restored |
| kache main @9531006 (n=3) | 0.37-0.38 s | B1 3.08 s; B2/B3 1.89-1.92 s | 0.77 s | B2/B3 `ignore` "automatic worktree seed" 216 ms; `rg` not seeded, 1.6 s |
| sccache 0.18.0, `CARGO_INCREMENTAL` unset | 5.31 s | 0.79 s | 0.81 s | 22 hits; non-cacheable: 8 "incremental", 11 "crate-type" |
| sccache 0.18.0, `CARGO_INCREMENTAL=0` | 4.71 s | 2.42 s | 2.34 s | 22 hits, 11 misses (workspace crates at a new path) |
| **Ferry-style fork** (n=3) | **0.06 s (0)** | **0.81-0.93 s** | 0.77-0.86 s | Upper layer 225 MB over a 366 MB base |
| Fork, base via `ferry-shim.sh` (ws to rustc, deps to kache main) | base 3.62 s | 0.85 s | 0.78 s | Agents ran plain cargo; base had 10 incremental sessions |
| Fork, base via kache main (default settings) | base 0.67 s (warm) | 3.07 s | n/a | Base had 0 incremental sessions |
| Fork, base and agents via kache with `KACHE_PRESERVE_INCREMENTAL=1` | base 3.53 s | 0.81 s | 0.77 s | State lives in `incremental.kache-preserved/` |

How the Ferry-style fork was built: unprivileged `unshare -Urm`. The worktree is bind-mounted at a
fixed path and the target is overlayfs over an immutable base target, mirroring
`research/cargo-fleet`. Unprivileged overlayfs in a userns worked on this kernel.

Caveats:
- ripgrep is small and this box is fast, so the absolute numbers are tiny. Read the ratios.
- One edit build in the "kache default base" run died with `ld ... signal 7 [Bus error]` because
  tmpfs was full. That row is discarded; the rerun with free space is clean.
- The shape matches the prototype on 2 vCPU: first edit is about 3-4x faster at a canonical path.

## 2. kache

### Identity (V)
- Repo: [kunobi-ninja/kache](https://github.com/kunobi-ninja/kache), made by Kunobi
  (GitHub org; repo created 2026-02-15, 1252 stars). License Apache-2.0.
- Crate owners on crates.io: Juan Leni (`jleni`, author of all 50 recent commits) and Emmanuel
  (`emmanuelm41`).
- Docs: [kunobi.ninja/docs/kache](https://kunobi.ninja/docs/kache); the source is in `docs/` in the repo.
- Latest release: **1.2.2**, 2026-10-15 ([crates.io](https://crates.io/crates/kache),
  [release](https://github.com/kunobi-ninja/kache/releases/tag/v1.2.2)), MSRV 1.95.
  - Very fast cadence: 1.0.0 shipped 2026-10-05 and about 25 versions since August.
  - The crate name dates from a 2021 0.1.0.
- Install:
  - `cargo install kache --locked` or `cargo binstall kache`.
  - Homebrew `kunobi-ninja/kunobi/kache`, APT, AUR, mise.
  - Static musl/darwin/windows tarballs on GitHub releases, with `.sha256` files.
  - [installation.mdx](https://github.com/kunobi-ninja/kache/blob/main/docs/getting-started/installation.mdx)
- Platforms: Linux, macOS, Windows. It also caches C/C++ and CUDA
  ([README](https://github.com/kunobi-ninja/kache/blob/main/README.md)).

### How it works (V unless marked)
- **Shape.** A `RUSTC_WRAPPER` plus a background daemon (`kache init` configures both). The
  content-addressed store is BLAKE3-keyed with zstd blobs.
  - Restores use reflink or CoW clones where the filesystem supports them, hardlinks or copies
    otherwise.
  - [architecture](https://github.com/kunobi-ninja/kache/blob/main/docs/how-it-works/architecture.mdx),
    [deduplication](https://github.com/kunobi-ninja/kache/blob/main/docs/deduplication.mdx)
- **Cache key.** It includes:
  - rustc and linker identity, crate name and types, cfg and features;
  - source contents from dep-info, flags, `-Z` flags, and `--extern` contents;
  - env reported by rustc, and `rerun-if-*` inputs of workspace build scripts.
  - [cache-key.mdx](https://github.com/kunobi-ninja/kache/blob/main/docs/how-it-works/cache-key.mdx)
- **Path normalization.** Known roots map to sentinels before hashing: workspace, `CARGO_HOME`,
  rustup home, tmp, the target dir, and `KACHE_BASE_DIR`/`paths.base_dirs`.
  - It also passes matching remap flags to rustc. On Linux, workspace paths become
    `/proc/self/cwd`.
  - Cargo's `-C metadata`/`extra-filename` hashes enter the key verbatim, but they are
    workspace-relative for members, so worktrees share entries.
- **What it caches.** rlibs, rmeta, **executables and test binaries**, and build-script runs
  (default on Linux and macOS). Restoring one replays the stored diagnostics.
- **New-target seeding** (`KACHE_SEED_NEW_TARGETS=1`, the default):
  - On Cargo's first `-vV` probe for an unbuilt target dir, the daemon copies registry and git
    units from the most recent same-rustc target, fingerprints included.
  - Workspace members are never copied, because their mtime freshness could pass as fresh.
  - It needs a running daemon.
  - **M:** seeding worked only with the daemon running. The daemon logged "seeded 2 registry
    units ... from .../wtB" and Cargo printed `Fresh memchr`, `Fresh itoa`.
  - [configuration.mdx: New checkouts](https://github.com/kunobi-ninja/kache/blob/main/docs/getting-started/configuration.mdx)
- **Remotes.** S3-compatible stores (`KACHE_S3_BUCKET/ENDPOINT/REGION/PREFIX/PROFILE`), GCS,
  a shared filesystem (`cache.remote.path`), and OCI registries. The remote is read after a local
  miss and written in the background. There is also an optional, preview "planner" prefetch
  service behind a Kunobi login. No network telemetry by default.
- **Concurrency.**
  - Same-key compiles across target dirs share one in-flight compile.
  - A memory-weighted scheduler paces rustc processes.
  - The docs say to give each live build its own target dir. `kache cargo -- build` keeps
    per-worktree build-dirs over a shared target on Cargo 1.91 and later.
  - [limits.mdx](https://github.com/kunobi-ninja/kache/blob/main/docs/how-it-works/limits.mdx)

### Incremental compilation (V, M)
- **Stripping is the default.** "Normal artifact-cached builds strip Cargo's incremental
  directory argument"
  ([quick-start](https://github.com/kunobi-ninja/kache/blob/v1.2.2/docs/getting-started/quick-start.mdx#executables-and-incremental-builds)).
  - Even direct passthroughs strip it, "to prevent APFS-related corruption in git worktrees on
    macOS" ([src/main.rs:1155 @9531006](https://github.com/kunobi-ninja/kache/blob/9531006/src/main.rs#L1155)).
  - **M:** a kache-built target has no `debug/incremental/<crate>` sessions, only
    `incremental.kache-auto/v1/<hash>/{state.json,unit.lock}`. This held for the default mode,
    `cache.exclude` and `cache.bypass_crates`.
- **Adaptive incremental** (`KACHE_ADAPTIVE_INCREMENTAL=1`, the default): a unit that is built,
  then misses in the same target, gets a private incremental dir under `incremental.kache-auto/`.
  It skips the cache for 8 builds or 15 minutes and never stores those results.
- **Forced incremental.** `KACHE_PRESERVE_INCREMENTAL=1` puts eligible units on incremental
  state, under `incremental.kache-preserved/`. `KACHE_INCREMENTAL_CRATES=a,b` does the same for
  the listed crates only.
- **Worktree incremental seeds, main only.** Introduced in
  [e39ab6f](https://github.com/kunobi-ninja/kache/commit/e39ab6f) (#1624, 2026-10-19) and made
  default in [22ff807](https://github.com/kunobi-ninja/kache/commit/22ff807) (#1647, 2026-10-21).
  - Requires rustc 1.98 or later. Controlled by `KACHE_SEED_INCREMENTAL` (default on) and
    `KACHE_INCREMENTAL_SEED_FROM`.
  - It imports a donor checkout's finalized sessions from up to 8 recent targets, with caps of
    1 GiB, 65,536 entries and 256 units per import.
  - It compiles with `-Zremap-cwd-prefix=/src`. On stable it sets `RUSTC_BOOTSTRAP=1` only in
    the rustc child, plus `-Zallow-features=`.
  - It rewrites `--out-dir`/`-o`/`--extern`/`-L` operands to cwd-relative paths
    ([worktree_seed.rs](https://github.com/kunobi-ninja/kache/blob/9531006/src/incremental_policy/worktree_seed.rs),
    [mutation-testing.mdx](https://github.com/kunobi-ninja/kache/blob/main/docs/getting-started/mutation-testing.mdx)).
  - **Not in 1.2.2:** `git merge-base --is-ancestor e39ab6f v1.2.2` is false.

### Exact harness usage (M, kache 1.2.2 and main@9531006)
```sh
export RUSTC_WRAPPER=/abs/kache            # main needs kache-engine in the same dir
export KACHE_CACHE_DIR=$H/kcache KACHE_RUNTIME_DIR=$H/krun KACHE_CONFIG=$H/empty.toml
export KACHE_HOST_CONFIG= XDG_CACHE_HOME=$H/xdg   # else probes land in $XDG_CACHE_HOME/kache
export KACHE_SOCKET_PATH=/tmp/kh.sock      # socket path must be <=107 bytes, or the daemon will not start
kache daemon start                         # needed for target seeding
cargo build --target-dir "$WT/target"      # one target dir per worktree
kache stats --full --last-build --root "$WT"
kache daemon stop
```
- Per-crate truth comes from `$KACHE_RUNTIME_DIR/events.jsonl`. Fields include `crate_name`,
  `root`, `result` (`miss`/`local_hit`/`passthrough`), `passthrough_reason`
  (`adaptive seed`, `adaptive active`, `automatic worktree seed`, `bypass rule: ...`) and
  `elapsed_ms`.
- Do not count `KACHE_PROGRESS=hits` lines: they printed "local hit" for units Cargo reported
  `Fresh` after seeding.
- kache main@9531006 still prints `kache 1.2.2`, so record the git rev.
- Disable caching per build with `KACHE_DISABLED=1`; skip the remote with `KACHE_LOCAL_ONLY=1`.

### Known limitations (V)
- Crates that read `CARGO_MANIFEST_DIR` via `env!` re-key per checkout.
- Proc macros that list directories are handled by a workspace tree-stamp check, which costs a
  dep-info pre-pass after changes. Workspaces with more than 20k entries are not stamped.
- Coverage and trybuild builds are path-local.
- On Linux the daemon only executes for clients with matching credentials and **namespaces**.
  Otherwise the client falls back to a local engine
  ([architecture](https://github.com/kunobi-ninja/kache/blob/main/docs/how-it-works/architecture.mdx#process-restrictions)).
- Example of a fixed cross-checkout correctness bug:
  [#1420](https://github.com/kunobi-ninja/kache/issues/1420) replayed a build-script run across
  checkouts with stale `rustc-env`.

## 3. sccache (V unless marked)
- **Version.** 0.18.0, 2026-09-16, Apache-2.0
  ([crates.io](https://crates.io/crates/sccache), [repo](https://github.com/mozilla/sccache)).
  main was at 2eeea91354 on 2026-10-22.
- **Incremental is refused.** `("incremental", _) => cannot_cache!("incremental")`
  ([rust.rs](https://github.com/mozilla/sccache/blob/v0.18.0/src/compiler/rust.rs#L1200-L1208)).
  The upstream tracking issue [#236](https://github.com/mozilla/sccache/issues/236) has been open
  since 2018. If `CARGO_INCREMENTAL=1` or `CARGO_BUILD_INCREMENTAL=1` is set, sccache exits with
  "incremental compilation is prohibited"
  ([commands.rs](https://github.com/mozilla/sccache/blob/v0.18.0/src/commands.rs#L902-L911)).
  **M:** reproduced.
- **Linker crates are not cached.** "Crates that invoke the system linker cannot be cached":
  bin, dylib, cdylib, proc-macro ([README](https://github.com/mozilla/sccache/blob/v0.18.0/README.md#rust)).
  **M:** 11 "crate-type" non-cacheable calls on ripgrep.
- **Absolute paths are in the key.**
  - The Rust hash includes every `CARGO_*` env var (except MAKEFLAGS, registries, jobs and
    encoded rustflags) and "9. The cwd of the compile"
    ([rust.rs](https://github.com/mozilla/sccache/blob/v0.18.0/src/compiler/rust.rs#L1554-L1595)).
  - `SCCACHE_BASEDIRS` exists, but it is only applied in the C/C++ paths. `rust.rs` has 0
    references on both v0.18.0 and main.
  - Open PRs to change this: [#2918](https://github.com/mozilla/sccache/pull/2918),
    [#2813](https://github.com/mozilla/sccache/pull/2813),
    [#2794](https://github.com/mozilla/sccache/pull/2794),
    [#2678](https://github.com/mozilla/sccache/pull/2678).
  - Related issues: [#2595](https://github.com/mozilla/sccache/issues/2595) "SCCACHE_BASEDIRS
    support for Worktree workflow" and [#2870](https://github.com/mozilla/sccache/issues/2870)
    (`CARGO_MANIFEST_DIR`/`OUT_DIR` are hashed verbatim).
- **Backends.** Local disk, S3/R2, Redis, Memcached, GCS, Azure, GitHub Actions, WebDAV,
  Alibaba OSS, Tencent COS, and a multi-level cache. There is also distributed compilation
  (`sccache-dist`).

## 4. Cargo-native (V unless marked)
- **Locking.**
  - Cargo takes an **exclusive** `<build-dir>/<profile>/.cargo-build-lock` for the whole build
    (labelled "build directory"). It is shared only under `-Zfine-grain-locking`
    ([#4282](https://github.com/rust-lang/cargo/issues/4282)).
  - It also takes a shared `.cargo-lock` for old/new Cargo interop, and an exclusive
    `.cargo-artifact-lock` on the artifact dir.
  - None of these are taken on NFS
    ([layout.rs @cad07cf](https://github.com/rust-lang/cargo/blob/cad07cf/src/compiler/layout.rs)).
    The message is `Blocking waiting for file lock on build directory`
    ([flock.rs](https://github.com/rust-lang/cargo/blob/cad07cf/src/util/flock.rs)).
  - The split lock landed in Cargo 1.96 ([#16708](https://github.com/rust-lang/cargo/pull/16708)).
  - **M:** two worktrees on one `CARGO_TARGET_DIR`, each rustc slowed by 2 s
    (`slowwrap.sh`): the second build printed the Blocking line and finished in 8.01 s, against
    8.50 s for the first. That is fully serialized.
  - The package-cache lock ("package cache") is separate. It is the only one the prototype saw
    under concurrency.
- **Fingerprints versus wrappers.**
  - The fingerprint's `rustc` field is `hash(rustc -vV)`. The `-vV` output is fetched *through*
    the wrapper
    ([fingerprint/mod.rs:1714](https://github.com/rust-lang/cargo/blob/cad07cf/src/compiler/fingerprint/mod.rs#L1714),
    [util/rustc.rs:63](https://github.com/rust-lang/cargo/blob/cad07cf/src/util/rustc.rs#L63)).
  - Only the `RUSTC_WORKSPACE_WRAPPER` path is mixed into `-C metadata`, and only for members
    ([compilation_files.rs:819](https://github.com/rust-lang/cargo/blob/cad07cf/src/compiler/build_runner/compilation_files.rs#L819)).
  - **M:** building without a wrapper, then with kache, then without again left identical
    `.fingerprint` names and everything `Fresh`.
- **build-dir.**
  - `build.build-dir`/`CARGO_BUILD_BUILD_DIR` split intermediates from final artifacts.
  - **Cargo 1.100 (release 2026-11-12) switches to the new per-unit build-dir layout**
    ([#17354](https://github.com/rust-lang/cargo/pull/17354), stabilized in
    [#16807](https://github.com/rust-lang/cargo/pull/16807)). The layout "is not a stable
    interface" ([build-cache doc](https://doc.rust-lang.org/nightly/cargo/reference/build-cache.html)).
- **Freshness.**
  - `-Zchecksum-freshness` is gaining `build.fingerprint = "content"`
    ([#14136](https://github.com/rust-lang/cargo/issues/14136),
    [#17382](https://github.com/rust-lang/cargo/pull/17382)). Build-script inputs still use mtimes.
  - `trim-paths` is stabilized for Rust 1.101 ([unstable.md](https://doc.rust-lang.org/nightly/cargo/reference/unstable.html)).
- **Shared cache work.**
  - 2026 goal "Cargo cross workspace cache" (owner @ranger-ross, champion @epage): an initial
    nightly cache for "basic crates (no build scripts, proc macros, etc)", with stabilization
    "likely not" in 2026
    ([goal](https://rust-lang.github.io/rust-project-goals/2026/cargo-cross-workspace-cache.html),
    [goals#626](https://github.com/rust-lang/goals/issues/626),
    [cargo#5931](https://github.com/rust-lang/cargo/issues/5931)).
  - Open PRs: [#17512](https://github.com/rust-lang/cargo/pull/17512) (blob dedup into
    `$CARGO_HOME/blobs` via reflink or hardlink),
    [#17624](https://github.com/rust-lang/cargo/pull/17624) (drop path-dep abspaths from
    `-Cmetadata`), [#17556](https://github.com/rust-lang/cargo/pull/17556) (trim-paths
    sanitizes fingerprints).
  - Also the 2026 goal "Incremental Systems Rethought"
    ([goal](https://rust-lang.github.io/rust-project-goals/2026/incremental-system-rethought.html)),
    about sharing incremental bases across invocations with different arguments.
- **Cleanup tools.** [cargo-sweep](https://github.com/holmgr/cargo-sweep) is marked unmaintained
  and points to cargo-clean-all. kache has `kache clean`/GC for target dirs.

## 5. Other prior art (V: repo/docs checked 2026-10-23)
- [Worktrunk](https://github.com/max-sixty/worktrunk): Rust, 9.4k stars, MIT/Apache.
  - `wt step copy-ignored` copies gitignored files (`target/`, `node_modules/`), optionally
    limited by `.worktreeinclude`. It reflinks on APFS, btrfs, XFS and ReFS and makes full
    copies on ext4 and NTFS ([docs](https://worktrunk.dev/step/#wt-step-copy-ignored)).
  - It keeps the *source* mtimes on copied files. Freshly checked-out workspace sources are
    therefore newer, "so those rebuild rather than reuse an artifact built from other sources"
    ([src/copy.rs](https://github.com/max-sixty/worktrunk/blob/main/src/copy.rs)).
  - Result: deps are reused, workspace crates rebuild, and there is no incremental reuse.
- [Claude Code worktrees](https://code.claude.com/docs/en/worktrees): created under
  `.claude/worktrees/<name>`.
  - `.worktreeinclude` copies gitignored files into each new worktree.
  - A `WorktreeCreate` hook "replace[s] the default `git worktree` logic entirely". It reads JSON
    on stdin and prints the directory, and `.worktreeinclude` is then not processed. There is
    also a `WorktreeRemove` hook.
- [coderabbitai/git-worktree-runner](https://github.com/coderabbitai/git-worktree-runner): Bash
  `git gtr`. It copies config and dirs (`includeDirs`) and runs setup. Nothing Cargo-specific.
- [dagger/container-use](https://github.com/dagger/container-use): an MCP server. Each agent gets
  its own container plus a git branch.
- [devflowinc/uzi](https://github.com/devflowinc/uzi): worktrees plus tmux; the last push was
  2025-06.
- [standardagents/dmux](https://github.com/standardagents/dmux): worktrees plus tmux.
- [stravu/crystal](https://github.com/stravu/crystal): "now Nimbalyst"; worktrees.
- [BloopAI/vibe-kanban](https://github.com/BloopAI/vibe-kanban): worktree workspaces.
- [Conductor](https://www.conductor.build/docs/concepts/workspaces-and-branches): a macOS app with
  git worktrees and setup scripts. None of these handle Cargo target dirs.
- Hermetic or remote action caches, one line each:
  - [Bazel remote caching](https://bazel.build/remote/caching), with
    [rules_rust](https://bazelbuild.github.io/rules_rust/).
  - [Buck2 remote execution](https://buck2.build/docs/users/remote_execution/).
  - Nix [crane](https://crane.dev/API.html) `buildDepsOnly`: a dependencies-only derivation that
    a binary cache can share.
  - [cargo-chef](https://github.com/LukeMathWalker/cargo-chef): `prepare`/`cook` for Docker-layer
    dependency caching.
  - All of these give crate- or action-level reuse across machines, and all need a non-Cargo
    build or a container flow. I did not verify their incremental behavior.

## 6. Comparison table

Legend: V = verified from source or docs, M = measured locally (section 1), I = inferred.

| Tool | Mechanism | Crate reuse across worktrees | Incremental reuse on first edit | Cross-machine | Safety vs shared-target bug | Platform |
|---|---|---|---|---|---|---|
| **Ferry** (prototype) | Immutable per-commit base, overlay/reflink fork, mtime stamp, canonical-path namespace | Full, 0 crates rebuilt, 0.06 s (M) | Yes: 0.81-0.93 s versus 0.76 s ideal (M) | No, local only (V: design) | Per-agent target dir; never shared (V: prototype) | Linux with userns (M); macOS crate-level only (I) |
| kache 1.2.2 | `RUSTC_WRAPPER` content-addressed store, path-normalized keys, daemon seeds registry units | Yes, 0.26-0.31 s (M) | No, incremental stripped, 3.1 s (V+M) | Yes: S3/GCS/fs/OCI (V) | Docs require a target per build; `kache cargo` splits build-dir (V) | Linux/macOS/Windows (V) |
| kache main (unreleased) | Above plus automatic incremental seeds (`-Zremap-cwd-prefix`, relative operands, child `RUSTC_BOOTSTRAP`) | Yes, 0.37 s (M) | Partial: only after a donor edited the crate; lib yes, bin no; 1.9 s versus 3.1 s (M) | Artifacts yes; seeds are local-only (V) | As above (V) | Seeds need rustc 1.98+ (V); other OS untested (I) |
| sccache 0.18.0 | `RUSTC_WRAPPER`, client/server, abs-path keys | Registry crates only; workspace crates miss or are non-cacheable; spawn 4.7-5.3 s (V+M) | Refuses incremental; local incremental after a slow spawn (V+M) | Yes, many backends (V) | Needs separate targets (I) | All (V) |
| Shared `CARGO_TARGET_DIR` | One target for all worktrees | Yes, but builds serialize on `.cargo-build-lock` (V+M) | Unclear across worktrees (I) | No | **Unsafe**: B can get A's artifacts with 0 rebuilt (V: prototype) | All |
| Copy target (`cp --reflink`, Worktrunk) | Clone `target/` into the new worktree | Deps yes; workspace crates rebuild unless stamped (V: Worktrunk); stamped means full (M) | No, path differs: 3.0 s (M) | No | Separate dirs (V) | CoW on APFS/btrfs/XFS/ReFS, else full copy (V) |
| Cargo cross-workspace cache | Planned user-wide unit cache | Not available; nightly target in 2026 (V) | n/a | Planned "future" remote (V) | n/a | n/a |
| Bazel / Buck2 / crane | Hermetic action or derivation cache | Yes, outside Cargo (I) | Not verified | Yes (V) | n/a | Various |

## 7. How kache and Ferry compose
- **Fingerprints are wrapper-independent.**
  - **V/M:** `RUSTC_WRAPPER` (kache) does not affect fingerprints or unit hashes. A base built
    with kache is `Fresh` for plain-cargo agents and vice versa.
  - **V:** `RUSTC_WORKSPACE_WRAPPER` *is* hashed for members, so it must match between base and
    agents.
  - **V:** a wrapper that changed `rustc -vV` output would invalidate everything.
- **A kache base loses incremental state (M).**
  - **V:** kache strips `-C incremental` on every path, including passthrough, `cache.exclude`
    and `cache.bypass_crates`. So `RUSTC_WRAPPER=kache` on the base destroys Ferry's main
    advantage.
  - **M:** first edit in the fork was 3.07 s against 0.85 s.
- **Working compositions (M):**
  1. **Dispatch shim** (`ferry-shim.sh`, 5 lines): if `CARGO_MANIFEST_DIR` is under the canonical
     workspace, `exec "$@"` (plain rustc, Cargo's incremental dir); otherwise exec kache.
     - Base build 3.62 s, against 7.70 s for a cold fork base. 10 incremental sessions.
     - Agents run plain cargo or the same shim: first edit 0.82-0.85 s.
     - kache then accelerates only dependencies, which is what crosses machines best.
  2. **`KACHE_PRESERVE_INCREMENTAL=1` on base and agents, both through kache**: first edit
     0.81 s. Agents are then locked into kache, because the state lives in
     `incremental.kache-preserved/`.
- **Running kache inside Ferry's namespace (I, risk).**
  - kache's daemon runs outside the mount namespace, and it tracks targets by path. Every
    Ferry agent shares the same canonical target path.
  - kache main refuses daemon delegation across namespaces (V) and falls back to a local engine.
    It worked in my runs.
  - Untested: target bookkeeping, orphan cleanup ("workspace deleted" when seen from outside
    the namespace), and donor selection when all agents share one path.

## 8. Could not verify
- Whether kache main's incremental seeds help a crate that no other worktree has edited yet.
  B1 got no seed. In Ferry's model the base has state for every member.
- Why the `rg` bin crate loses incremental reuse at a different path even with relative operands
  and `-Zremap-cwd-prefix`. kache main shows the same gap. Candidates (I): absolute
  `OUT_DIR`/`CARGO_MANIFEST_DIR` env values tracked by the dep graph, or linker arguments.
- The APFS incremental-corruption rationale in kache's `main.rs`: no linked issue found.
- Conductor internals (closed source); the incremental behavior of Bazel, Buck2 and crane.
- Behavior on large workspaces (300+ crates) and on macOS. Everything here is ripgrep on Linux
  tmpfs.

## Design implications for Ferry
1. **Reposition the kache comparison; do not just say "kache has no incremental".** That is true
   of 1.2.2 but stale for kache main. Claim what still holds when measured:
   - 0-crate spawn (0.06 s versus 0.3-0.4 s);
   - incremental first edit for *every* workspace crate, including bins, with no donor needed
     (0.85 s versus 1.9 s, or 3.1 s with no donor);
   - no `RUSTC_BOOTSTRAP` or unstable flags.

   Re-run the harness on each kache release after 1.2.2 and cite version plus git rev.
2. **Harness rules:**
   - Pin and record `kache --version` *and* the git rev.
   - Start the daemon. Isolate `KACHE_CACHE_DIR`, `KACHE_RUNTIME_DIR`, `KACHE_CONFIG`,
     `KACHE_HOST_CONFIG=` and `XDG_CACHE_HOME`, and use a short `KACHE_SOCKET_PATH`.
   - Warm the store from a separate worktree.
   - Report B1 (no donor) and B2+ (donor) separately.
   - Read per-crate data from `events.jsonl`, never from `KACHE_PROGRESS` lines.
   - For sccache, leave `CARGO_INCREMENTAL` unset (`=1` makes it exit), and report both unset
     and `=0`.
   - Metrics: spawn time, first edit, second edit, crates compiled (`cargo build -v`
     Fresh/Compiling), and target or upper-layer disk delta.
3. **Offer kache as an optional base accelerator through a dispatch shim**, not as
   `RUSTC_WRAPPER=kache`.
   - Ferry owns `RUSTC_WRAPPER`. Workspace members go to plain rustc; everything else goes to
     `$FERRY_DEP_WRAPPER`, which can be kache or sccache.
   - Keep `RUSTC_WORKSPACE_WRAPPER` and `RUSTFLAGS` identical between base and agents.
   - Detect and warn when a user-level `build.rustc-wrapper` points at kache.
4. **A namespace-free fallback for macOS and for Linux without userns.**
   - A Ferry rustc wrapper that makes `--out-dir`/`-o`/`-L`/`--extern` cwd-relative, plus
     `-Zremap-cwd-prefix`, gave lib-crate incremental reuse at a different path: `ignore` 208 ms
     against 220 ms at the same path. First edit was 1.75 s against 3.0 s.
   - It needs `RUSTC_BOOTSTRAP=1`, which should be scoped to the rustc child with
     `-Zallow-features=` as kache does, and it does not fix bin crates. Treat it as opt-in
     degraded mode and keep the namespace path primary.
5. **Never share a target dir.** Besides the mtime bug, Cargo serializes whole builds on
   `.cargo-build-lock`. Ferry's per-agent target with a canonical path sidesteps both. Document
   this in the README with the measured Blocking repro.
6. **Treat `target/` as opaque**, as the overlay and reflink design already does.
   - Cargo 1.100 (2026-11-12) changes the build-dir layout.
   - Test Ferry against 1.100 beta now, and avoid parsing the layout.
   - If Ferry ever splits `build-dir`, fork the build-dir rather than the artifact dir.
7. **Track `-Zchecksum-freshness` / `build.fingerprint="content"`** as the replacement for mtime
   stamping. Build scripts still use mtimes, so keep stamping until that changes.
8. **Agent integration points.**
   - Ship a Claude Code `WorktreeCreate`/`WorktreeRemove` hook: `ferry spawn` prints the path.
     Note that `.worktreeinclude` is skipped under a hook, so Ferry must copy those files itself.
   - Ship a Worktrunk post-start recipe. Present Worktrunk's `copy-ignored` as the closest prior
     art: crate-level reuse for deps, no incremental reuse.
9. **Overlay copy-up cost is visible.** A 225 MB upper layer over a 366 MB base after two edits,
   matching the prototype. Prefer a reflink-fs lower layer or `split-debuginfo` before claiming
   disk efficiency against kache. kache shares blocks with its store via reflinks and dedups.

## Open questions
- Will kache's next release ship worktree incremental seeds on by default, and on which
  platforms? Does it plan bin-crate seeding?
- Can the bin-crate incremental gap at a different path be closed without namespaces? Test
  relativizing `OUT_DIR`-derived and env-tracked paths. Does `trim-paths` (stable in 1.101) help?
- Safe kache settings for use inside the namespace: `KACHE_SEED_NEW_TARGETS=0`,
  `KACHE_AUTO_CLEAN_ORPHANED_TARGETS=0`? This needs a test with several agents sharing one
  canonical target path.
- Do the ratios hold on a 300+ crate workspace and on macOS APFS (clonefile, no namespaces)?
- Will Cargo's cross-workspace cache (#5931, #17512) make Ferry's crate-level layer redundant on
  nightly in 2026, and how should Ferry use it?
