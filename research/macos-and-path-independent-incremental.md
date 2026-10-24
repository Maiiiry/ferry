# macOS support and path-independent rustc incremental state

Research brief for Ferry, 2026-10-23. Linux experiments: NixOS, kernel 6.18, 12 cores, tmpfs,
rustc/cargo **1.99.0 stable** (b940084d7), nightly 1.101.0 (7189d39d9, 2026-10-22), ripgrep @
`6892bc2`, debug `cargo build`. macOS facts come from Apple sources. **Nothing here was run on a Mac.**

## TL;DR

1. Exactly three things make a warm target lose incremental reuse at a new path. All three are
   verified in rustc source and confirmed by experiment:
   - **(a)** `working_dir` is a `[TRACKED]` option. If it differs, rustc discards the whole cache.
   - **(b)** `--out-dir` is absolute. It is hashed through the `output_filenames` query, which
     debuginfo reads for every CGU, so every CGU goes red.
   - **(c)** Leaf crates only (bin/dylib/cdylib/staticlib): on targets with
     `emit_debug_gdb_scripts` (Linux yes, **Apple no**), every CGU reads `used_crate_source`. That
     query holds canonical absolute rlib paths, so all of the bin's CGUs go red.
2. A `RUSTC_WORKSPACE_WRAPPER` can fix (a) and (b) with no mount namespace. It adds
   `-Zremap-cwd-prefix=<constant>` and rewrites absolute worktree/target paths in rustc's argv to
   relative ones. Libs then get canonical-path-level CGU reuse at any path (215/216 CGUs). On a
   target with gdb scripts off, the bin gets full reuse too (75/75 vs 0/75, A/B with custom target
   specs). On ripgrep with debug=0, the result equals the canonical path: 0.80 s vs 0.83 s.
3. Explicit `--remap-path-prefix=$PWD=…` and Cargo's `trim-paths` do **not** work. The absolute
   FROM side of the flag is itself tracked. Symlinks don't work either: rustc's `getcwd` is always
   physical.
4. On macOS: clone the base target with `copyfile(COPYFILE_CLONE|COPYFILE_RECURSIVE)` (or
   `clonefile` on the directory, with caveats). Add mtime stamping for crate-level reuse, and the
   wrapper for incremental reuse. Macs have no per-process path virtualization, so the wrapper is
   the only way to get the incremental win there. It is a **prediction to verify on a real Mac**.

## A. Which absolute paths matter (VERIFIED, rustc 1.99.0 source)

| Input | Dep-tracking | Effect at a new path | Source |
|---|---|---|---|
| `working_dir` (cwd, after remapping) | `[TRACKED]` | whole cache discarded | [options.rs L433-435](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_session/src/options.rs#L433-L435), computed in [config.rs L2999-3012](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_session/src/config.rs#L2999-L3012) |
| `--remap-path-prefix` FROM=TO list | `[TRACKED_NO_CRATE_HASH]` | per-fork FROM discards cache | [options.rs L393](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_session/src/options.rs#L393) |
| `--remap-path-scope` (stable since 1.95, [#147611](https://github.com/rust-lang/rust/pull/147611)) | `[TRACKED_NO_CRATE_HASH]` | must be `all` (see RealFileName) | options.rs L395 |
| `-Zremap-cwd-prefix=TO` | `[TRACKED]`, but only TO is stored | constant across forks | [options.rs L2791](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_session/src/options.rs#L2791); cwd applied when the mapping is built, "so the absolute cwd is never stored in the tracked `remap_path_prefix` option (#132132)": [config.rs L1416-1433](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_session/src/config.rs#L1416-L1433), landed in [#157348](https://github.com/rust-lang/rust/pull/157348) (merged 2026-06-12, **1.98.0**) |
| `-C incremental=<dir>` | `[UNTRACKED]` (top-level and `-C`) | none | options.rs L362, L2240 |
| `-L`, `--extern` | `[UNTRACKED]` options... | ...but see `used_crate_source` below | options.rs L347, L367 |
| `unstable_features` (`RUSTC_BOOTSTRAP`) | `[TRACKED]` | base and forks must agree | options.rs; measured below |

The options hash is compared on load. On mismatch, rustc logs `completely ignoring cache because
of differing commandline arguments` ([load.rs L98-112](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_incremental/src/persist/load.rs#L98-L112)).

**Source-file identity.** `StableSourceFileId` = hash(FileName [+ crate id])
([lib.rs L2132-2153](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_span/src/lib.rs#L2132-L2153)).
`RealFileName` stores `{name, working_directory, embeddable_name}`. Its `Hash` skips the local
(absolute) part **only if the name was remapped in all scopes**
([lib.rs L327-339](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_span/src/lib.rs#L327-L339)).
So without a full remap of the cwd, every source file's id embeds the absolute cwd, even for
relative names. On metadata export, fully remapped names drop the local part
([`update_for_crate_metadata`](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_span/src/lib.rs#L477-L485)).
Measured: the rebuilt `libignore-*.rmeta` was **byte-identical (md5)** whether built at the
canonical path or a fork path.

**Query-level leaks (cache loads, but CGUs go red):**
- `output_filenames` is `feedable` and stable-hashed
  ([queries.rs L2471](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_middle/src/queries.rs#L2471)).
  `OutputFilenames.out_directory` is absolute because Cargo passes `--out-dir /abs/target/...`
  ([config.rs L1160](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_session/src/config.rs#L1160)).
  Every CGU with debuginfo reads it in
  [`build_compile_unit_di_node`](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_codegen_llvm/src/debuginfo/metadata.rs#L873-L921).
- `used_crate_source`/`crate_extern_paths` are `eval_always` and return canonicalized absolute
  paths ([queries.rs L2054, L2334](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_middle/src/queries.rs#L2334)).
  `debuginfo_finalize` calls `get_or_insert_gdb_debug_scripts_section_global` for every CGU of
  leaf crates ([mod.rs L735-741](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_codegen_llvm/src/debuginfo/mod.rs#L735-L741),
  [gdb.rs L83-114](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_codegen_llvm/src/debuginfo/gdb.rs#L83-L114)).
  That function calls `collect_debugger_visualizers_transitive`, which reads
  `used_crate_source(cnum)` for every crate
  ([base.rs L640-658](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_codegen_ssa/src/base.rs#L640-L658)).
  This is gated on `target.emit_debug_gdb_scripts`, which is **`false` for all Apple targets**
  ([apple/mod.rs L167](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_target/src/spec/base/apple/mod.rs#L167))
  and `true` by default ([spec/mod.rs L2946](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_target/src/spec/mod.rs#L2946)).
- Split-DWARF name: only used where `debuginfo_kind == Dwarf`. It is not used on Apple targets
  (`DwarfDsym`) ([session.rs L657](https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_session/src/session.rs#L657)).

**How Cargo passes paths** ([util/workspace.rs L128-152](https://github.com/rust-lang/cargo/blob/rust-1.99.0/src/util/workspace.rs#L128-L152)):
- Path (workspace) packages get a source path relative to the workspace root, with
  `cwd = workspace root`.
- Registry and git packages get an absolute source path, with `cwd = package root`.
- `--out-dir`, `-C incremental`, `-L dependency=` and `--extern` are all absolute.
- `profile.trim-paths` passes `--remap-path-prefix=<abs ws root>=.` and
  `<abs build-dir>=/cargo/build-dir`, both with absolute FROM
  ([trim_paths.rs](https://github.com/rust-lang/cargo/blob/rust-1.99.0/src/compiler/trim_paths.rs#L91-L230)).
  It is stabilized in cargo [#17488](https://github.com/rust-lang/cargo/pull/17488) (merged
  2026-10-06, so expected in 1.101; nightly already accepts it without `-Z`). It is still
  path-dependent for incremental, because of the absolute FROM.
- Upstream proposal [#157405](https://github.com/rust-lang/rust/pull/157405) (draft PoC, open)
  would add a `{cwd}` placeholder to the **stable** `--remap-path-prefix`. That would remove the
  need for `-Z` and `RUSTC_BOOTSTRAP`. The tracking issues for
  [`-Zremap-cwd-prefix`](https://github.com/rust-lang/rust/issues/89434) and
  [#132132](https://github.com/rust-lang/rust/issues/132132) are still open.

## B. Experiment: first edit after fork (ripgrep)

**Method.**
- Build a base, fork it, append a documented `pub fn` to `crates/ignore/src/lib.rs`, then time
  `cargo build --message-format=json`.
- A fork is a `cp -a` copy (mtimes preserved), or an overlayfs mount at a new path in
  `unshare -rm`. Overlay was used for the later repetitions to save tmpfs space. It adds copy-up
  cost.
- **Units** = `compiler-artifact` with `fresh:false`.
- **CGU reused** = `.o` work products in the new session dir that kept their pre-build inode
  (cp -a runs) or pre-build mtime (overlay runs), out of the total.
- `-Zincremental-info` was on for all wrapper runs, so `RUSTC_BOOTSTRAP=1` was set for them.
- After every run, a no-op rebuild re-ran **0 units** (Cargo freshness intact).

| Variant (stable 1.99, debug=2 unless noted) | First edit, s (runs) | Median | Units | CGU reused ignore / rg | rustc cache |
|---|---|---|---|---|---|
| 1 same path (canonical) | 0.78, 0.94, 0.91 | **0.91** | 2 | 215/216, 256/256 | reused |
| 2 different path, no flags | 3.12, 2.98 (cp); 4.17, 3.91 (ovl) | **3.5** | 2 | 0/216, 0/256 | "completely ignoring" |
| 3 wrapper: `--remap-path-prefix=$PWD=/ferry/src` | 3.06 (n=1) | | 2 | 0/216, 0/256 | ignored (FROM tracked) |
| 4a wrapper: `-Zremap-cwd-prefix` only | 2.35 (n=1) | | 2 | 0/216, 0/256 | **loaded**, all CGUs red (out-dir) |
| 4b wrapper: relativize argv only | 2.98 (n=1) | | 2 | 0/216, 0/256 | ignored (`working_dir`) |
| **4c wrapper: `-Zremap-cwd-prefix` + relativize** | 1.70, 1.80, 1.83 | **1.80** | 2 | **215/216**, 0/256 | loaded; bin red (gdb scripts) |
| 4d = 4c, src moved but target at same abs path | 0.87 (n=1) | | 2 | 215/216, 256/256 | full reuse |
| 5 debug=0, different path, no flags | 2.33, 2.33 | 2.33 | 2 | 0/216, 0/256 | ignored |
| 6 debug=0, 4c at **same** path (baseline) | 0.84, 0.83 | 0.83 | 2 | 215/216, 256/256 | reused |
| **7 debug=0, 4c at different path** | 0.69, 0.82, 0.80 | **0.80** | 2 | **215/216, 256/256** | full reuse |
| 7' = 7 with `-Zremap-cwd-prefix=.` | 0.88, 0.85 | 0.87 | 2 | 215/216, 256/256 | full reuse |

Medians are over the listed runs. Several rows are n=1 or n=2, because I could not delete fork
directories (see Process notes). The CGU counts are deterministic, and the conclusions rest on
them. Ratio: canonical vs different path is ~3.8x here, against ~3x in RESULTS.md on 2 vCPU.

Mechanism checks (tiny 2-crate workspace, lib + bin):
- **Symlink trick fails (both forms).** Measured: `cd <symlink>; cargo build` and
  `cargo build --manifest-path <symlink>/Cargo.toml`. With `--manifest-path`, Cargo keeps the
  logical path as the workspace root, but rustc's `getcwd()` is physical (wrapper logged
  `PHYS=…/fork`), so the log shows "completely ignoring cache". POSIX `getcwd` "shall contain
  no components that … are symbolic links"
  ([POSIX](https://pubs.opengroup.org/onlinepubs/9799919799/functions/getcwd.html)). `PWD` is
  irrelevant.
- **Cargo `trim-paths="all"`** (nightly 1.101, no `-Z` needed) at a different path: cache
  ignored. Measured args: `--remap-path-prefix=<abs fork>=.` and `<abs fork>/target=/cargo/build-dir`.
  It also wrote `tinybin.trim-paths.json` (unremap file).
- **gdb-scripts A/B.** Custom target specs identical to `x86_64-unknown-linux-gnu` except
  `"emit-debug-gdb-scripts": false`, built with `-Zbuild-std`, wrapper 4c, different path:
  the bin reused **0/75** CGUs with gdb scripts on and **75/75** with them off. This is the best
  available proxy for Apple targets.
- **`RUSTC_BOOTSTRAP` flip.** Base built without it, fork built with it, at the same path:
  cache ignored (2.24 s, 2.26 s; 0/216 + 0/256).
- `-C incremental` path: in 4a it was absolute and different, yet the dep graph loaded. This
  confirms `[UNTRACKED]`.

**Correctness of 4c (measured unless noted):**
- rustc dep-info (`deps/*.d`) lists real relative paths (`crates/ignore/src/lib.rs`), so Cargo
  freshness works: the no-op re-ran 0 units, and a second edit rebuilt the bin.
- `file!()` / panic `Location` for workspace members are relative (`crates/ignore/src/walk.rs`,
  697 such strings in `rg`). That is the same as without remapping, because Cargo already passes
  relative paths. Registry crate paths are unchanged (absolute `~/.cargo/registry/...`).
- DWARF `DW_AT_comp_dir` = the remap target (`/ferry/src`, measured with readelf). Line tables
  are relative, so debuggers need `settings set target.source-map /ferry/src <worktree>` (lldb) or
  `set substitute-path` (gdb). With `-Zremap-cwd-prefix=.` (row 7'), comp_dir is `.` and sources
  resolve when the debugger runs from the worktree root. Cargo trim-paths chose `.` for this
  reason ([#17488](https://github.com/rust-lang/cargo/pull/17488)).
- `env!("CARGO_MANIFEST_DIR")` stays the real absolute fork path. A test asserting
  `Cargo.toml` exists there passed (`cargo test --workspace`, tiny workspace). Affected items go
  red locally, which is correct.
- Build-script `OUT_DIR` files are only remapped if the target dir is under cwd. Otherwise those
  generated files are path-dependent (partial loss, still correct). *Not measured.*
- Rewriting argv to relative paths means rustc's JSON artifact notifications carry relative
  paths. Cargo build, test and run all worked; `cargo doc`, `--timings`, build scripts that parse
  args, and proc macros were *not tested*.
- Soundness relies on rustc hashing every path-dependent input. That is the same assumption
  remapped CI caches make. Path-related bugs have occurred (e.g.
  [#148328](https://github.com/rust-lang/rust/issues/148328), now closed).

**Cargo integration (VERIFIED, docs and source):**
- `RUSTC_WORKSPACE_WRAPPER` applies to workspace members only and nests as
  `$RUSTC_WRAPPER $RUSTC_WORKSPACE_WRAPPER $RUSTC`, so it is sccache-compatible
  ([config docs](https://doc.rust-lang.org/cargo/reference/config.html#buildrustc-workspace-wrapper)).
- Its **path** is hashed into workspace-member filenames
  ([compilation_files.rs L798-802](https://github.com/rust-lang/cargo/blob/rust-1.99.0/src/compiler/build_runner/compilation_files.rs#L798-L802)).
  So wrapper and plain builds never share artifacts, and the wrapper needs a stable absolute
  path.
- `cargo clippy` sets its own workspace wrapper.

## C. APFS cloning (VERIFIED from Apple sources; behavior not run on hardware)

**`clonefile(2)`** ([xnu man page](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/clonefile.2), [`sys/clonefile.h`](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/clonefile.h)):
- **Directories.** "If src names a directory, the directory hierarchy is cloned as if each item
  was cloned individually." The LIMITATIONS section says: "Cloning directories with these
  functions is strongly discouraged. Use copyfile(3) to clone directories instead."
- **Atomicity.** "expected to be atomic i.e. … all new objects being created successfully or no
  new objects will be created."
- **Attributes.** The clone has "its own copy of attributes and extended attributes which are
  identical to those of" the source, with these exceptions:
  - ownership is set to the current user, unless running as root (`CLONE_NOOWNERCOPY`);
  - setuid/setgid bits are cleared;
  - ACLs are inherited from the target directory unless `CLONE_ACL` is passed.
  So mtimes and modes carry over (inferred from "identical attributes"; not tested).
- **Flags.** `CLONE_NOFOLLOW`=0x1 (only for non-directory sources), `CLONE_NOOWNERCOPY`=0x2,
  `CLONE_ACL`=0x4, `CLONE_NOFOLLOW_ANY`=0x8, `CLONE_RESOLVE_BENEATH`=0x10.
- **Errors.**
  - `EEXIST` if dst exists.
  - `EXDEV` if src and dst are on different filesystems.
  - `ENOTSUP` if the filesystem doesn't support it.
  - `ENOSPC`, including on later CoW overwrites.
  - `EDEADLK` for dataless files (iCloud).
- **Support detection.** `getattrlist` `ATTR_VOL_CAPABILITIES` → `VOL_CAP_INT_CLONE`.
- **Hard links** are not mentioned. "As if each item was cloned individually" implies links
  become independent CoW files (UNVERIFIED). Measured locally: the ripgrep debug target has 1418
  files, 111 dirs, 38 multiply-linked files (e.g. `debug/rg` ↔ `deps/rg-*`), 367 MB.

**Why directory clones are discouraged (Apple DTS, Kevin Elliott):** the clone "Prevent[s] all
changes to the source hierarchy while the operation is in progress … in the worst case, you could
theoretically panic the kernel." His numbers: 250 files, clonefile 0.008 s vs copyItem 0.070 s,
and directory cloning "~10x" faster than `copyfile(COPYFILE_CLONE)`
([thread 784446](https://developer.apple.com/forums/thread/784446),
[thread 818372](https://developer.apple.com/forums/thread/818372)).
For Ferry the source is an immutable base nobody writes to, so the lock is harmless.
Extrapolating from his numbers (UNVERIFIED): copyfile ~0.4 s for ripgrep's ~1500 entries, and
~10 s+ for a 50k-file target.

**`copyfile(3)`** ([man page](https://github.com/apple-oss-distributions/copyfile/blob/main/copyfile.3)):
- `COPYFILE_CLONE` (1<<24) is best-effort: it falls back to copying, and is equivalent to
  `EXCL|STAT|XATTR|DATA|NOFOLLOW_SRC`.
- With `COPYFILE_RECURSIVE` (1<<15) it walks the tree with fts, clones each file, and **copies
  (creates) directories**. Symlinks are not followed. The destination must not exist.
- `COPYFILE_CLONE_FORCE` (1<<25) errors on directories.
- It is not atomic. Default recursion crosses mount points (`COPYFILE_STATE_FORBID_CROSS_MOUNT`).

**Rust bindings:**
- **libc 0.2.191** ([apple/mod.rs](https://github.com/rust-lang/libc/blob/libc-0.2/src/unix/bsd/apple/mod.rs)):
  - `pub fn clonefile(src: *const c_char, dst: *const c_char, flags: u32) -> c_int;`
  - `clonefileat(src_dirfd, src, dst_dirfd, dst, flags: u32)`
  - `fclonefileat(srcfd, dst_dirfd, dst, flags: u32)`
  - `copyfile(from, to, state: copyfile_state_t, flags: copyfile_flags_t /*u32*/)`, `fcopyfile`
  - `COPYFILE_*`, `VOL_CAP_INT_CLONE`, and `statfs.f_fstypename: [c_char; 16]`
  - **No `CLONE_*` constants**: define them locally from the header.
- **rustix 1.1.5**: only `rustix::fs::fclonefileat(src: impl AsFd, dst_dir: impl AsFd, dst, CloneFlags)`
  with `CloneFlags::{NOFOLLOW, NOOWNERCOPY}` ([at.rs](https://github.com/bytecodealliance/rustix/blob/main/src/fs/at.rs#L447-L462)),
  plus `fcopyfile`.
- `std::fs::copy` on macOS already does `fclonefileat` then falls back to `fcopyfile`, per file
  ([unix.rs L2203-2240](https://github.com/rust-lang/rust/blob/1.99.0/library/std/src/sys/fs/unix.rs#L2203-L2240)).
- **APFS detection:** `statfs` with `f_fstypename == "apfs"` (field verified; the string value
  is not checked on hardware). It is better to test `VOL_CAP_INT_CLONE`, or to just try the
  clone and fall back on `ENOTSUP`/`EXDEV`.

## D. Canonical-path options on macOS (evaluated)

| Option | Per-process? | Privilege | Verdict |
|---|---|---|---|
| `/etc/synthetic.conf` firmlinks/symlinks ([man](https://keith.github.io/xcode-man-pages/synthetic.conf.5.html)) | no, global; root dir only | root, and removal needs a reboot | one fixed global path, so one agent at a time |
| `chroot` | yes | root, plus a full toolchain inside | no |
| `sandbox-exec` ([man](https://keith.github.io/xcode-man-pages/sandbox-exec.1.html)) | yes | none | **DEPRECATED**; allow/deny rules only, no path remap |
| macFUSE kext / bindfs | no, global mounts | Apple Silicon needs "Reduced Security" in Recovery ([wiki](https://github.com/macfuse/macfuse/wiki/Getting-Started)) | no |
| macFUSE FSKit backend (macOS 15.4+) | no, global | none | no kext, but slower I/O ([wiki](https://github.com/macfuse/macfuse/wiki/FUSE-Backends)); "only /Volumes" mount restriction UNVERIFIED |
| Loopback NFS ([macbind](https://github.com/progrium/macbind), PoC) | no, global | unprivileged `mount_nfs` | up to 2 s attribute staleness; no |
| One agent at a time per canonical path | n/a | none | serializes builds; defeats the purpose |
| Per-slot paths (N fixed dirs, each with a base built *at that path*) | n/a | none | works, but costs N workspace-member rebuilds per base commit. Unnecessary if the wrapper works on Apple |

Conclusion: macOS has no per-process path view without root or kexts. **Ship path independence
(wrapper), not path virtualization.**

## E. macOS gotchas

- **Timestamps.** APFS stores ns since 1970
  ([Eclectic Light](https://eclecticlight.co/2025/10/24/be-careful-when-interpreting-apfs-timestamps/)).
  Stamping to a fixed old mtime works as on Linux. HFS+/exFAT external volumes are coarser, and
  clonefile gives `ENOTSUP` there.
- **Case sensitivity.** The default APFS volume is case-insensitive. GitHub's macOS runners are
  too (per [runner-images#865](https://github.com/actions/runner-images/issues/865), a feature
  request; secondary). Paths differing only in case collide in worktrees.
- **Where to store things.**
  - `~/Library/Caches` holds "cached data that can be regenerated … Apps should never rely on the
    existence of cache files" ([Apple, archived](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/MacOSXDirectories/MacOSXDirectories.html)).
    Fine for bases and target dirs; **not** for agent worktrees, which hold user work.
  - Put worktrees under `~/Library/Application Support/ferry` or next to the repo.
  - Everything must be on the same volume as the base for cloning (`EXDEV`).
- **Backups and indexing.** Cargo already excludes target dirs from Time Machine and iCloud on
  macOS, writes `CACHEDIR.TAG`, and does no Spotlight exclusion
  ([cargo-util paths.rs](https://github.com/rust-lang/cargo/blob/rust-1.99.0/crates/cargo-util/src/paths.rs#L789-L870)).
  Ferry should apply the same exclusion to its own root.
- **Debuginfo layout.** Cargo's dev default on macOS is `split-debuginfo=unpacked`
  ([profiles](https://doc.rust-lang.org/cargo/reference/profiles.html#split-debuginfo)): DWARF
  stays in `.o` files, and the binary holds linker-written absolute OSO paths. "OSO entries on
  macOS are not remapped" is listed as open in [rust#111540](https://github.com/rust-lang/rust/issues/111540).
  So a fork's binary points at its own `.o` files after relink. A not-yet-relinked cloned binary
  points at the *base's* objects, which is fine while the base is immutable and not yet GC'd.
- **Gatekeeper.** `com.apple.quarantine` is set by the downloading app. curl does not set it, so
  `curl | sh` installs aren't Gatekeeper-checked, while browser-downloaded unsigned binaries are
  blocked (secondary sources, e.g. [dev.to](https://dev.to/hiyoyok/handling-macos-gatekeeper-as-an-unsigned-indie-dev-the-xattr-struggle-1028)).
  arm64 code must be signed; the linker ad-hoc signs automatically
  ([Eclectic Light](https://eclecticlight.co/2020/08/22/apple-silicon-macs-will-require-signed-code/)).
  Re-sign with `codesign -s -` after any post-link modification. Notarization needs a paid
  Developer ID.
- **CI.** `macos-latest` = macOS 26 arm64
  ([runner-images](https://github.com/actions/runner-images/blob/main/README.md)): 3 vCPU M1,
  7 GB RAM, 14 GB SSD. Intel `macos-26-intel`: 4 vCPU, 14 GB
  ([docs](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)). Two
  warm ripgrep targets fit; large workspaces may not.
- **Linux too.** Ubuntu 24.04 allows unprivileged user namespaces but denies capabilities inside
  them ([release notes](https://documentation.ubuntu.com/release-notes/24.04/)). So
  `unshare -rm` + `mount` fails there by default, and the wrapper is also the Linux fallback.

## Design implications for Ferry

1. **Make `ferry-rustc` (a `RUSTC_WORKSPACE_WRAPPER` shim) a first-class mode,** default on
   macOS, and the fallback on Linux when the namespace setup fails. The rules:
   - Add `-Zremap-cwd-prefix=.`. On stable, also set `RUSTC_BOOTSTRAP=1` in the wrapper's own
     child env only.
   - Rewrite every argv occurrence of `<worktree>/` to relative, and `<target>` to its path
     relative to cwd. This covers `--out-dir`, `-C incremental=`, `-L` and `--extern`.
   - Only touch invocations whose cwd is the worktree root.
   - Keep `--remap-path-scope` at its default (`all`).
2. **Layout:** keep the relative path from worktree to target identical across agents and the
   base, e.g. `<agent>/src` + `<agent>/target` (`../target`), or target inside the worktree.
   Build the **base through the same shim, same flags, same `RUSTC_BOOTSTRAP` value**; any
   mismatch silently discards the cache (measured).
3. **Install the shim at a stable absolute path** (e.g. `~/.ferry/bin/ferry-rustc`). Cargo
   hashes the path into filenames, so a Homebrew Cellar path that changes on upgrade would
   rebuild all workspace members. Chain any existing `RUSTC_WORKSPACE_WRAPPER`; leave
   `RUSTC_WRAPPER` (sccache) alone.
4. **Gate by version:** require rustc ≥ 1.98 (#157348). Below that, `-Zremap-cwd-prefix` stores
   the absolute cwd and gives nothing. Feature-detect `{cwd}` in `--remap-path-prefix` (#157405)
   and prefer it when it lands, which drops `RUSTC_BOOTSTRAP`.
5. **If the user enables `profile.trim-paths`,** have the shim translate
   `--remap-path-prefix=<cwd>=X` into `-Zremap-cwd-prefix=X`, and drop or relativize the
   `<build-dir>=/cargo/build-dir` rule. Otherwise trim-paths breaks fork reuse.
6. **macOS fork:** `copyfile(src, dst, NULL, COPYFILE_CLONE | COPYFILE_RECURSIVE)` by default
   (Apple-sanctioned, never blocks writers), then stamp mtimes. Optionally use `clonefile(dir)`
   on the immutable base for ~10x faster forks; it is safe because nothing writes the base. Fall
   back to a parallel copy on `ENOTSUP`/`EXDEV`.
   - Use raw `libc` (`copyfile`, `clonefile`, local `CLONE_*` consts). rustix lacks path-based
     clonefile.
   - The fork is not atomic with copyfile: write into `<dst>.tmp` and `rename(2)`.
7. **Document the platform difference honestly:**
   - Linux namespaces: canonical paths, full CGU reuse for libs and bins.
   - macOS shim (predicted): same reuse, *verify in CI*.
   - Linux shim fallback: libs full reuse, bins with debuginfo none (~2x not ~4x on ripgrep),
     until rustc stops reading `used_crate_source` in `collect_debugger_visualizers_transitive`.
     That is a small upstream PR worth filing.
8. **Debugging:** with prefix `.`, run lldb/gdb from the worktree root, or have Ferry print a
   `target.source-map` line. `env!("CARGO_MANIFEST_DIR")` and test fixtures keep real paths.
9. **Add a macOS CI job (macos-latest)** that runs this brief's ripgrep protocol: fork via clone
   at a new path, edit, then assert CGU reuse with the inode/mtime counter. This is the single
   missing verification.

## Open questions

- Does wrapper mode actually give full bin CGU reuse on Apple (DwarfDsym, unpacked, ld64)? It is
  predicted by source and the gdb A/B, but not run on a Mac.
- clonefile/copyfile behavior on real targets: mtime preservation, hard-link splitting, and time
  for 10k-100k-file targets (`clonefile(dir)` vs `copyfile` recursive) on M1 CI. The
  "77k files ≈ 2 min" figure from a search summary is UNVERIFIED.
- Is `RUSTC_BOOTSTRAP=1` scoped to workspace members acceptable to users and policy? Any crates
  in the wild that change behavior under it?
- Untested argv-rewrite interactions: `cargo doc` / rustdoc (`RUSTDOC_WRAPPER`), `--timings`,
  `cargo llvm-cov`, proc macros reading paths, `include!(concat!(env!("OUT_DIR"),…))` with the
  target outside the worktree.
- Does Cargo's Time Machine exclusion survive a clone (it is stored as metadata on the directory)?
- Upstream: will #157405 (`{cwd}` placeholder) pass its MCP, and would rustc accept untracking
  `used_crate_source` for gdb visualizers (fixes Linux bins)?

## Process notes

- **Leftover data (cleanup denied).** The permission system denied `rm -rf` of my scratch build
  dirs mid-session, so I did not delete anything (and did not try workarounds). Leftovers:
  `…/scratchpad/research-macos/{v,ov,tiny,rsrc}`, about 8.6 GB on tmpfs `/tmp`. Overlay `ov/*/w/work`
  dirs were created in a user namespace and may need `unshare -r rm -rf` to remove. This
  constrained repetition counts (n=1-4).
- One `crates.io` API request mistakenly carried the user's email in its User-Agent header. Later
  requests used a neutral UA.
