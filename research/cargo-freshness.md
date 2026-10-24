# Cargo freshness, fingerprints, build-dir, shared cache (status 2026-10-23)

Environment for every "measured" claim: NixOS, kernel 6.18, tmpfs, stable `cargo 1.99.0 (5f94df478 2026-08-27)` /
`rustc 1.99.0`, nightly `cargo 1.101.0-nightly (15be64aa0 2026-10-20)` / `rustc 7189d39d9 2026-10-22`.
Note that stable 1.99.1 is out ([releases.rs](https://releases.rs/)); I tested on 1.99.0. Source links are pinned to cargo tag
[`0.100.0`](https://github.com/rust-lang/cargo/tree/0.100.0) (the cargo crate is versioned minor+1, so 0.100.0 is the
code behind Cargo 1.99) and to master `cad07cf8e` (2026-10-23).
**The path `src/cargo/core/compiler/fingerprint/` no longer exists.** Commit `071048184` ("Elevate compiler to top-level
mod", 2026-07-17) moved it to `src/compiler/fingerprint/`, and Cargo 1.99 already ships the new path.

Test workspace (6 members, 7 compile units): `leaf` <- `mid` <- `app` (bin, prints a marker from `leaf`), plus
`pathy` (`env!("CARGO_MANIFEST_DIR")` and `include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/data.txt"))`), `wsrel`
(`include_str!("../../ws_data.txt")`, a file outside the package but inside the workspace) and `bsdir` (`build.rs` with
`cargo::rerun-if-changed=assets`, a directory with a nested subdirectory). "Rebuilt n/7" counts
`compiler-artifact` messages with `"fresh":false` from `cargo build -v --message-format=json`. Dirty reasons are quoted from
the `-v` "Dirty ..." lines.

## TL;DR

- **Checksum freshness is still unstable** (nightly only, tracking issue open). Since nightly 2026-09-02 it takes **two**
  switches: `-Zchecksum-freshness` **and** `build.fingerprint = "content"`. Either one alone gives you mtime freshness.
- Content mode covers **only rustc dep-info inputs**. Build-script `rerun-if-changed` inputs, build scripts with no
  `rerun-if` lines, rustdoc, and the check that compares a unit's outputs against its dependencies' outputs all stay
  mtime-based. Switching the mode in either direction rebuilds everything.
- **Content mode does not make a shared target dir safe.** Measured: B gets its own Rust code but still gets **A's
  build-script output**, and A and B keep rebuilding each other's artifacts.
- Unit hashes are path-independent (measured identical at two paths). But on a relocated or copied target,
  **`env!("CARGO_MANIFEST_DIR")` is silently stale**. On <=1.100, an `include_str!` of a workspace file outside the package
  **keeps watching the original location** (measured: the edit was missed, 0 rebuilt). Nightly fixes the second problem, not the first.
- `rerun-if-changed=<dir>` takes the **recursive max mtime of the dir itself, every subdir and every file**. Ferry must
  stamp directories, and must **never re-stamp after the agent starts**, or deleted files go unnoticed (measured stale output).
- If a user config sets `build.build-dir`, then **`CARGO_TARGET_DIR` alone leaks all intermediates into the user's
  build-dir**. Inside Ferry's canonical mount, `{workspace-path-hash}` is **the same for every agent** (measured). Ferry must set
  `CARGO_BUILD_BUILD_DIR`.
- None of the in-flight Cargo work (new layout in 1.100, fine-grain locking, cross-workspace cache) gives per-agent
  incremental state or isolation. Nothing makes Ferry obsolete in the 2026 timeframe.

## 1. Checksum freshness status (VERIFIED)

| Question | Answer | Source |
|---|---|---|
| Stabilized? | **No.** [#14136](https://github.com/rust-lang/cargo/issues/14136) is open, labeled `S-waiting-on-feedback`. A call for testing went out 2026-09-03 ("We are preparing to stabilize a new `build.fingerprint` config field"). I found no stabilization PR as of 2026-10-23. The 1.100 changelog does not stabilize it. | issue comments; [Cargo changelog](https://doc.rust-lang.org/nightly/cargo/CHANGELOG.html) |
| Long-term plan | Stabilize the config, then change the default to `content` (or a hybrid) only after the perf work in [#14722](https://github.com/rust-lang/cargo/issues/14722). The mtime code paths stay because of build scripts. | epage, #14136, 2026-08-07 and 2026-08-18 |
| Shape change | [#17382](https://github.com/rust-lang/cargo/pull/17382) (merged 2026-08-21, so in 1.100 beta and nightly) made `-Z` a gate only. The method now comes from `build.fingerprint`. | [master compiler/mod.rs#L220](https://github.com/rust-lang/cargo/blob/cad07cf8e597397fff5d0f6cb79c9f870c0b1dd2/src/compiler/mod.rs#L220) |
| Enable (nightly >= 2026-09-02) | Config: `[unstable] checksum-freshness = true` plus `[build] fingerprint = "content"`. CLI: `-Zchecksum-freshness` plus `CARGO_BUILD_FINGERPRINT=content`. Env only: `CARGO_UNSTABLE_CHECKSUM_FRESHNESS=true CARGO_BUILD_FINGERPRINT=content`. **Measured: all three work.** | [unstable.md](https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#checksum-freshness) |
| Only one of the two switches | **Measured:** `-Zchecksum-freshness` alone means mtime (7/7 rebuilt against a content-built target). `CARGO_BUILD_FINGERPRINT=content` alone gives `warning: ignoring build.fingerprint = "content" without -Zchecksum-freshness` and mtime. | same |
| Cargo <= 1.99 | `-Zchecksum-freshness` alone enables it, and there is no `build.fingerprint`. | [0.100.0 fingerprint/mod.rs](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/mod.rs) (`checksum: cli_unstable().checksum_freshness`) |
| Needs nightly rustc? | **Yes.** Cargo passes `-Z checksum-hash-algorithm=blake3` to rustc and rustdoc ([compiler/mod.rs#L834](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/mod.rs#L834)). `rustc +nightly -Z help` lists `checksum-hash-algorithm=val (blake3 or sha256)`, and stable has no `-C` equivalent (measured). The rustc side landed in [rust#126930](https://github.com/rust-lang/rust/pull/126930). | |
| Switching rebuilds? | **Yes, every unit, in both directions** (the method is part of the hashed `LocalFingerprint::CheckDepInfo`). Measured on nightly, content to mtime: 7/7, `the prior compilation fingerprinted build inputs using content and this one used mtime`. Measured on stable+`RUSTC_BOOTSTRAP`, mtime to content: 7/7, `checksum freshness requested, prior compilation did not use checksum freshness`. | |

What content mode covers (from source, consistent with the experiments below):

| Input kind | Freshness method in content mode |
|---|---|
| Files in rustc dep-info: sources, `include_str!`/`include_bytes!`, generated `OUT_DIR` files that are `include!`d | **content**: size first, then blake3 ([find_stale_file](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/mod.rs#L2083)) |
| `# env-dep:` vars from dep-info (`env!`, `option_env!`) | by value, in both modes. Exception: vars that Cargo itself sets on the rustc command (`CARGO_MANIFEST_DIR`, `OUT_DIR`, ...) are **dropped** unless they are tracked manifest metadata ([dep_info.rs#L338](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/dep_info.rs#L338)) |
| Build-script `rerun-if-changed` paths | **always mtime** ([mod.rs#L1007](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/mod.rs#L1007)), and recursive for dirs (section 4) |
| Build script with no `rerun-if-*` lines; rustdoc without `-Zrustdoc-depinfo` | `Precalculated("{newest mtime} ({relpath})")`, so the **mtime value itself is hashed** ([sources/path.rs#L202](https://github.com/rust-lang/cargo/blob/0.100.0/src/sources/path.rs#L202)) |
| A unit's outputs vs its dependencies' outputs | **always mtime** (`dep_mtime > max_mtime`, [mod.rs#L1335](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/mod.rs#L1335)). [#17626](https://github.com/rust-lang/cargo/issues/17626) (opened 2026-10-23) proposes output checksums so build-dirs can share hardlinks. |
| Registry and git deps | no file check at all: paths under `$CARGO_HOME/{registry,git}` are skipped, and their sources are not in dep-info |

## 2. Experiments

### 2a. Fork a warm target into a second worktree at a different path

Setup: build the base worktree, wait about 1 s, `git worktree add` at another path (fresh checkout mtimes), `cp -a` the target, build.

| Toolchain / mode | Rebuilt | Why |
|---|---|---|
| nightly, mtime (default) | **7/7** | every source is newer than the base build |
| nightly, `content` | **2/7** (`bsdir`, `app`) | `the file bsdir/assets has changed`: the rerun-if-changed dir is still checked by mtime |
| nightly, `content` + stamp only `bsdir/assets/**` files and all tracked dirs | **0/7** | the Rust sources keep their fresh-checkout mtimes and are still fresh |
| stable 1.99, mtime + Ferry-style stamp of tracked **files only** | **2/7** | same `bsdir/assets` dir mtime (`1s after last build`) |
| stable 1.99, mtime + stamp tracked files **and their parent dirs** | **0/7** | |

### 2b. Shared `CARGO_TARGET_DIR` contamination

Sequence: A builds the base. B edits the leaf marker to `ONLY_IN_B` and writes `assets/a.txt=a-from-B`. One second later A
edits to `ONLY_IN_A` / `a-from-A`. A builds, then B builds.

- **Stable 1.99, mtime: reproduced.** A rebuilt 4/7. B rebuilt **0/7**, and B's binary printed `mid:ONLY_IN_A` and
  `a.txt=a-from-A`. `grep -a -c ONLY_IN_A libleaf-*.rlib` returned 1, while B's source said `ONLY_IN_B`.
- **Nightly, content:** B rebuilt 3/7 (`leaf/src/lib.rs has changed (checksum didn't match, blake3=...)`), and the binary
  printed `mid:ONLY_IN_B`, **but still `assets=a.txt=a-from-A`**. The build-script output stays contaminated. A's next
  build rebuilt 3/7 again, so the two agents keep rebuilding each other's work. **Content mode narrows the bug class but does
  not close it.**

## 3. Path sensitivity on stable 1.99 (VERIFIED)

**Same commit at two absolute paths of different depth, each built from scratch:** identical `-C metadata`,
`-C extra-filename`, `.fingerprint/` dir names, fingerprint hash file contents, `deps/` filenames and `build/` dir names for all
7 units (measured with `diff`). The rlib bytes differ (`libleaf-*.rlib` differs at byte 76: embedded absolute paths). Why the
hashes match: the fingerprint docs say "we always hash relative path names"
([mod.rs#L302](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/mod.rs#L302)), the fingerprint uses
`path: hash_u64(path_args(ws, unit).0)` (relative to the workspace,
[#L1693](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/mod.rs#L1693)), and the metadata uses
`package_id().stable_hash(ws_root)`
([compilation_files.rs](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/build_runner/compilation_files.rs)).

**Relocating a built workspace with mtimes preserved:**

| Crate | `cp -a` to new path (old path still exists) | `mv` (old path gone) |
|---|---|---|
| leaf, mid, app | fresh | fresh |
| `pathy`: `env!("CARGO_MANIFEST_DIR")` | fresh; **binary prints the OLD path** | fresh; prints the old, now nonexistent path, even after other crates rebuild |
| `pathy`: `include_str!` of an absolute path **inside** the package | stored package-relative, so it follows the copy. Editing the copy rebuilds correctly. | same |
| `wsrel`: `include_str!("../../ws_data.txt")`, outside the package but inside the workspace | stored **absolute**, so it watches the ORIGINAL file. **Editing the copy's `ws_data.txt` rebuilt 0/7 and the binary still printed `wsdata v1`.** | rebuild: `the file .../ws/wsrel/src/../../ws_data.txt is missing` |

Nightly includes [#17573](https://github.com/rust-lang/cargo/pull/17573) (merged 2026-10-09; **not** in the 1.100 beta),
which stores such paths workspace-relative. Measured on nightly: editing the copy gives `the file ws_data.txt has changed`
and a correct rebuild. `CARGO_MANIFEST_DIR` is still stale on nightly (it printed `.../orig/pathy` from the copy). Paths
outside the workspace are always stored absolute.

**Precise mtime rule:**
1. Reference timestamp for a compile unit: the mtime of the fingerprint dep-info file
   (`<build-dir>/debug/.fingerprint/<pkg>-<hash>/dep-<kind>-<target>`, or `build/<pkg>/<hash>/fingerprint/` in the 1.100
   layout). Cargo writes `invoked.timestamp` just before spawning rustc
   ([compiler/mod.rs#L435](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/mod.rs#L435)) and, on success, sets the
   dep-info file's mtime to that timestamp ([#L525](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/mod.rs#L525)).
   Measured: `dep-lib-leaf` = `invoked.timestamp` = 1792802652.632362127, while `.rlib`/`.rmeta` are later (real write time).
2. **A source is stale iff `source_mtime > reference` (strictly). Equal counts as fresh:**
   `if path_mtime <= reference_mtime { continue; }`
   ([mod.rs#L2199](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/mod.rs#L2199); the comment cites #5918).
   Measured: source set to exactly the reference mtime (ns) gave 0 rebuilt. +1 ns gave `has changed (... 1ns after last build ...)`.
3. A unit is also stale if any dependency's newest output is **strictly newer** than this unit's newest output (only the
   rmeta for pipelined edges). This uses real mtimes in both modes
   ([#L1335](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/mod.rs#L1335)).
4. Build-script runs: the reference is the run's `output` file, rewound to the invocation start
   ([custom_build.rs](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/custom_build.rs)). The paths are compared with `mtime_recursive`.

## 4. `rerun-if-changed` on a directory (VERIFIED)

Source: [`paths::mtime_recursive`](https://github.com/rust-lang/cargo/blob/0.100.0/crates/cargo-util/src/paths.rs#L300). For a
directory it walks the tree with `walkdir(follow_links=true)` and takes the **max mtime over the directory itself, every
subdirectory and every file**. For symlinks it takes max(link mtime, target mtime). An empty dir uses its own mtime.

Measured on stable 1.99, starting from a stamped tree at 0/7:

| Action | Result |
|---|---|
| `touch bsdir/assets/sub` (nested dir only, no content change) | build script reran (`the file bsdir/assets has changed`) |
| `touch bsdir/assets/sub/b.txt` | reran |
| create an untracked `assets/sub/new.txt`, then re-stamp the tracked files and dirs | reran (the new file keeps a fresh mtime) |
| **delete `new.txt`, then re-stamp the tracked files and dirs** | **0/7 rebuilt; the binary still lists `new.txt=n` (STALE)** |

A deletion only shows up as a directory mtime change, so re-stamping a directory hides it.

Build scripts **without** `rerun-if` lines hash the newest package file's mtime *value* and relative path. If several files
tie on mtime (as a uniformly stamped tree does), <=1.100 keeps whichever file it lists first. [#17521](https://github.com/rust-lang/cargo/pull/17521)
(merged 2026-10-16, nightly only) breaks ties by path. I did not reproduce a spurious rerun from this.

## 5. `build.build-dir` (VERIFIED)

- Stabilized in **Cargo 1.91** (2025-10-30, [#15833](https://github.com/rust-lang/cargo/pull/15833)).
  Env var: **`CARGO_BUILD_BUILD_DIR`**. Default: the target-dir (`build_dir.or_else(target_dir)`,
  [workspace.rs#L500](https://github.com/rust-lang/cargo/blob/0.100.0/src/workspace/workspace.rs#L500)). Cargo scripts
  default to `{cargo-cache-home}/build/{workspace-path-hash}`.
  [config doc](https://doc.rust-lang.org/cargo/reference/config.html#buildbuild-dir)
- Template variables: `{workspace-root}`, `{cargo-cache-home}` (= `CARGO_HOME`) and `{workspace-path-hash}`. The last is a hash of
  the **canonicalized manifest path**, split as `xx/yyyyyyyyyyyyyy`
  ([context/mod.rs#L775](https://github.com/rust-lang/cargo/blob/0.100.0/src/context/mod.rs#L775)). Measured: templates work in
  the env var too (`CARGO_BUILD_BUILD_DIR='{workspace-root}/bd3'` created `ws/bd3`).
- What goes where (measured, 1.99, old layout):
  - target-dir: `CACHEDIR.TAG` and `debug/{app, app.d, lib*.rlib, lib*.d, examples/, .cargo-lock, .cargo-artifact-lock}`.
  - build-dir: `.rustc_info.json`, `CACHEDIR.TAG` and `debug/{.fingerprint, deps, build, incremental, examples, .cargo-build-lock}`.
  - In the 1.100 layout, the build-dir holds `debug/build/<pkg>/<hash>/{out,fingerprint,run}` plus `incremental/`
    ([build-cache doc](https://doc.rust-lang.org/nightly/cargo/reference/build-cache.html)).
- Precedence (measured):
  - A project `.cargo/config.toml` with `build-dir = ".../userbd/{workspace-path-hash}"` plus only `CARGO_TARGET_DIR` sends
    final artifacts to Ferry's dir but **all fingerprints, deps and incremental data to `userbd/22/e5c85ae145c6cc`**.
  - Adding `CARGO_BUILD_BUILD_DIR` means nothing is written to `userbd`.
  - A CLI `--config build.build-dir=...` **beats** the env var.
- **Collision under Ferry's canonical mount (measured):** two different clones, each bind-mounted at the same path inside
  `unshare -Urm`, both resolved `{workspace-path-hash}` to `userbd2/71/d6d26765db664e`. Outside the namespace they got `b1/...`
  and `e9/...`. So a user's templated build-dir combined with Ferry's canonical path gives a shared build-dir, which is the
  section 2b bug.

## 6. Related Cargo work (VERIFIED status)

- **New build-dir layout:** re-stabilized in [#17354](https://github.com/rust-lang/cargo/pull/17354) (after the
  [#17187](https://github.com/rust-lang/cargo/pull/17187) rollback) and ships in **1.100 on 2026-11-12**. It is already the
  nightly default; `-Zbuild-dir-new-layout` now only warns that it is stabilized. A private escape hatch exists:
  `__CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT=1`. Goal page:
  [2025h2/cargo-build-dir-layout](https://goals.rust-lang.org/2025h2/cargo-build-dir-layout.html). The layout "is not a stable interface".
- **Fine-grain locking:** `-Zfine-grain-locking` (nightly, [#4282](https://github.com/rust-lang/cargo/issues/4282)) uses
  unit-level locks and implies the new layout. It lets concurrent cargo processes share one build-dir, but freshness is
  unchanged, so the contamination bug remains.
- **Cross-workspace cache:** 2026 goal [cargo-cross-workspace-cache](https://goals.rust-lang.org/2026/cargo-cross-workspace-cache.html)
  ([goals#626](https://github.com/rust-lang/goals/issues/626), [#5931](https://github.com/rust-lang/cargo/issues/5931)).
  - The plan starts with only "basic crates (no build scripts, proc macros, etc)".
  - The Oct 14 update describes a design doc and CAS "blob storage". The aim was something on nightly in October, and nothing
    was announced as of Oct 14.
  - Dedup across build-dirs is tracked in [#17453](https://github.com/rust-lang/cargo/issues/17453), with initial PR
    [#17512](https://github.com/rust-lang/cargo/pull/17512) open. The update notes "deduplication across multiple Git
    worktrees ... popular in AI workflows".
  - epage on #17453: "We'll need to defer mutable packages", so workspace members are out of scope at first.
- Also relevant:
  - [2026/target-dir-size](https://goals.rust-lang.org/2026/target-dir-size.html) names multi-worktree AI workflows.
  - [#17556](https://github.com/rust-lang/cargo/pull/17556) (open) would sanitize absolute paths in fingerprints under trim-paths.
  - [#17626](https://github.com/rust-lang/cargo/issues/17626) (open) proposes output checksums.
- **Nothing here makes Ferry obsolete.** The planned cache targets immutable units and has no per-agent incremental state, and
  rustc incremental data embeds absolute paths. A future CAS could cut Ferry's disk use, but hardlinked blobs share one mtime
  per inode, which is exactly the problem #17626 is about.

## 7. Wrappers, RUSTFLAGS, RUSTC_BOOTSTRAP

- **`RUSTC_WRAPPER` does not change fingerprints if the wrapper passes `-vV` through.** Measured with a pass-through
  `exec "$@"` wrapper: 0/7 rebuilt when turning it on and 0/7 when turning it off. The fingerprint's `rustc` field hashes
  the `rustc -vV` output, and Cargo runs `-vV` *through* the wrapper
  ([util/rustc.rs#L67](https://github.com/rust-lang/cargo/blob/0.100.0/src/util/rustc.rs#L67)). The wrapper path only goes into
  the `.rustc_info.json` cache key. A wrapper that rewrites `-vV` output would invalidate everything (inferred from the code).
- **`RUSTC_WORKSPACE_WRAPPER` does change things:** it is hashed into the metadata of workspace members
  ([compilation_files.rs#L800](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/build_runner/compilation_files.rs#L800)).
  Measured: 7/7 rebuilt with it, 0/7 after removing it (both artifact sets coexist).
- **RUSTFLAGS: yes, a change invalidates every unit that receives the flags.** Without `--target` that is all units. With
  `--target`, build scripts and proc-macros don't receive them. The flags are in `Fingerprint.rustflags`
  ([#L1702](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/fingerprint/mod.rs#L1702)) and in `unit_id` /
  `-C extra-filename` ([#L858](https://github.com/rust-lang/cargo/blob/0.100.0/src/compiler/build_runner/compilation_files.rs#L858)),
  but not in `-C metadata`.
  - Measured: `--cfg=ferry_probe` rebuilt 7/7; back to no flags, 0/7; the flag again, 0/7. The builds coexist as
    `leaf-eee22a...` and `leaf-73674a...`, both with `metadata=5aec029f50ef29ee`.
  - Unexpected: `--remap-path-prefix` RUSTFLAGS also got a distinct `extra-filename` on 1.99, even though the code comment
    says such flags are excluded. I did not dig into why.
- **`RUSTC_BOOTSTRAP=1`** (documented only, not a recommendation):
  - Without it, stable cargo rejects `-Z` ("only accepted on the nightly channel").
  - With it, `cargo 1.99 -Zchecksum-freshness` works, because rustc inherits the variable and accepts `-Z checksum-hash-algorithm`.
  - Toggling `RUSTC_BOOTSTRAP` alone: **0/7 rebuilt** (measured; it is not in dep-info env-deps for these crates). I did not test crates that use `#![feature]`.

## Design implications for Ferry

1. **Always export both `CARGO_TARGET_DIR=/wtarget` and `CARGO_BUILD_BUILD_DIR=/wtarget`** for base and agent builds, so
   one forked dir holds everything. Ferry's canonical path makes `{workspace-path-hash}` identical across agents, so never rely on
   user config. `ferry doctor` should warn if `.cargo/config.toml` sets `build.build-dir`/`target-dir`, or if an agent passes
   `--target-dir` or `--config build.*dir` (the CLI beats env).
2. **Stamp directories too.** At spawn, stamp every tracked path's ancestor directories, up to the worktree root, with the same
   constant as files (`touch -h`). Only stamp a directory whose entry list equals the base's, which is the directory analogue
   of "content equals base".
3. **Never re-stamp a live agent tree** (no stamping on `ferry sync`/rebase after the agent has edited). Stamping hides
   deletions and mtime bumps, and the result is a stale build-script output (measured). A rebase should re-fork from a new base.
4. **Freeze the stamp constant forever.** Old-style build scripts and rustdoc hash the mtime value, so changing `@1700000000`
   reruns them in every fork.
5. **Never offer a shared target dir, even with content freshness.** Build-script inputs stay contaminated and agents keep
   rebuilding each other's work (measured).
6. **Forks must preserve mtimes** (`cp -a` / `--reflink --preserve=timestamps`; overlay copy-up preserves them). Rule 3 of
   section 3 compares real output mtimes in both modes.
7. **Treat the canonical path as a correctness feature, not just perf.** In the non-namespace mode (macOS), a forked target
   embeds the base's `CARGO_MANIFEST_DIR`, so tests that locate fixtures that way would read the *base's* files. On <=1.100,
   workspace-relative `include_str!` watches the base. Mitigation for that mode: scan the base's rustc `.d` files (in
   `deps/` or `build/*/out`) for `# env-dep:CARGO_MANIFEST_DIR=`, `OUT_DIR`, or any path under the base root. Then delete those
   units' fingerprint dirs in the fork to force a rebuild. This is cheap and layout-agnostic if Ferry globs for the `.d` files.
8. **Pin the fingerprint inputs between base and agents:**
   - toolchain (`rustc -vV`), `RUSTFLAGS`/`CARGO_ENCODED_RUSTFLAGS`/`target.*.rustflags`, `RUSTC_WORKSPACE_WRAPPER`,
     profile/features, linker config, `[env]`, and env vars read by `env!`;
   - the freshness mode: `CARGO_UNSTABLE_CHECKSUM_FRESHNESS` + `CARGO_BUILD_FINGERPRINT` must match, or every unit rebuilds.

   Record them in the base's `ready` file and warn on mismatch. `RUSTC_WRAPPER` (sccache/kache) may differ safely.
9. **Offer `content` freshness as an opt-in only on nightly** (env pair above; `-Zchecksum-freshness` alone on <=1.99
   nightlies). It removes the need to stamp Rust sources but **not** build-script inputs, so keep stamping. Stamping then
   becomes belt-and-braces for `.rs` files and load-bearing for build scripts.
10. **Diagnostics for free:** run builds with `-v` and parse the `Dirty <pkg>: <reason>` lines, or use
    `CARGO_LOG=cargo::compiler::fingerprint=info` (the module path changed in 1.99). Report "reused n/m units" from
    `--message-format=json` `fresh` flags. This is what `ferry doctor` should print when reuse drops.
11. **Don't parse build-dir internals** (the layout changes in 1.100). Treat the dir as opaque and fork it whole. Anything that
    must inspect units should go through JSON messages or glob both layouts.

## Open questions

- When will `build.fingerprint` stabilize? There was a call for testing on 2026-09-03 and no FCP/PR found by 2026-10-23.
  Watch [#14136](https://github.com/rust-lang/cargo/issues/14136) and [#14722](https://github.com/rust-lang/cargo/issues/14722).
- If Cargo adopts hardlinked CAS blobs ([#17453](https://github.com/rust-lang/cargo/issues/17453)/[#17512](https://github.com/rust-lang/cargo/pull/17512)),
  how do they interact with Ferry's overlay lower layer and reflink forks (shared inode mtime, in-place writers such as mold)? Not tested.
- Do mtime ties in build scripts without `rerun-if` lines cause spurious reruns on <=1.100 in large stamped repos? Not reproduced here.
- Does `RUSTC_BOOTSTRAP` show up as an env-dep for crates that use `#![feature]`? Not tested.
- Why did `--remap-path-prefix` RUSTFLAGS still change `-C extra-filename` on 1.99? Not investigated.
- Files read by proc-macros (`include_dir!`, sqlx) appear in dep-info only via unstable tracked-path APIs, so they are probably
  invisible to both freshness modes. Not tested.
- I did not measure content-mode overhead on a large workspace (ripgrep-size or bigger).
