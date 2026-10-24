# Path remapping without namespaces

**Experimental.** `remap-paths` recovers most of rustc's incremental reuse
for agents that build without the namespace sandbox: on macOS, and on Linux
where user namespaces are unavailable. It is off by default.

## The problem it solves

Without the namespace sandbox, each agent builds at its own path. rustc's
incremental cache is tied to absolute paths in two ways:

- the working directory is a tracked option: if it differs, rustc discards
  the whole cache;
- the absolute `--out-dir` that Cargo passes is hashed into every codegen
  unit that has debug info, so even a loaded cache is invalid.

So the first edit of a crate in a new agent recompiles it from scratch. Cargo
itself is fine with the move (it hashes workspace paths relatively); Ferry
still rebuilds the workspace's own crates once after spawn, because code can
capture absolute paths where Cargo cannot see it (see
[How it works](how-it-works.md#canonical-paths-keeping-incremental-state)).

## What it does

With `remap-paths`, the base and every agent build through `ferry-rustc`, a
`RUSTC_WORKSPACE_WRAPPER` that is a link to the ferry binary in
`<store parent>/bin`. Cargo uses a workspace wrapper only for workspace
members. For each member compile that uses incremental compilation, the
wrapper:

- adds `-Zremap-cwd-prefix=.`, so the absolute working directory never enters
  rustc's tracked options;
- sets `RUSTC_BOOTSTRAP=1` in that rustc process only, because `-Z` flags are
  otherwise nightly-only;
- rewrites absolute paths under the worktree and the target dir in rustc's
  arguments (`--out-dir`, `-C incremental=`, `-L`, `--extern`, ...) into
  relative ones. `--remap-path-prefix` rules are left alone.

Builds reach the target dir through a `.ferry-target` link inside the source
root, so the relative path from sources to target is the same in the base and
every agent. Ferry hides the link from `git status` through the repository's
`.git/info/exclude`. Every other rustc call, including Cargo's version probes,
passes through unchanged.

## Enabling it

```toml
[sandbox]
remap-paths = true
```

or `FERRY_REMAP_PATHS=1`, then build a new base with `ferry base` and spawn
agents from it. Notes:

- It needs rustc 1.98 or newer (the release with
  [rust-lang/rust#157348](https://github.com/rust-lang/rust/pull/157348); before
  it, `-Zremap-cwd-prefix` still stored the absolute working directory). With an
  older toolchain, `ferry base` warns `remap-paths disabled: ...` and builds
  without it.
- It only applies to bases built without the namespace sandbox. On Linux with
  working user namespaces, the sandbox is used and this setting has no effect;
  set `sandbox = "off"` as well to try remapping there.
- Whether a base uses remapping is part of its identity, and its agents always
  match it: a mismatch in the wrapper or in `RUSTC_BOOTSTRAP` makes rustc
  discard the cache.

## Measured results

Ferry's test `tests/remap.rs` builds a workspace with a library and a binary,
copies the source tree and the target dir to new paths, deletes the
originals, edits both crates and rebuilds. It counts codegen units whose
object files rustc reused. On Linux with rustc 1.99.0:

| Crate | With `ferry-rustc` | Without |
|---|---|---|
| library | 12 of 13 codegen units reused | 0 of 13 |
| binary, default debug info | 0 of 19 | 0 of 19 |
| binary, `debug = 0` | 17 of 19 | 0 of 19 |

Binaries with debug info get nothing on Linux because every codegen unit of a
binary embeds a gdb scripts section, and computing it reads the absolute
paths of all its dependencies. Apple targets do not emit that section, so on
macOS binaries are predicted to be reused like libraries. That prediction
comes from rustc's source and an experiment with custom Linux target specs
during Ferry's research; **it has not been verified on a real Mac.**

A second test runs the same edit through Ferry itself: `ferry base` and
`ferry spawn` with the sandbox off, the edit, then the agent's first build.
On Linux with rustc 1.99.0 it reuses the same 12 of 13 library units (0 of 13
without remapping). CI runs the whole test suite in remap mode on macOS 26,
where the binary prediction above is asserted and a further test checks that
debug info and backtraces survive remapping. Its results will replace the
prediction here once CI has run them.

## Limitations

- **`RUSTC_BOOTSTRAP=1`** is set for workspace-member compiles. It lets those
  compiles use unstable flags on a stable toolchain, which some policies
  forbid, and code that detects nightly through it may behave differently.
- **Stable wrapper path.** Cargo hashes the workspace wrapper's path into
  member artifact names. Builds with and without the wrapper do not share
  artifacts, and moving the store parent rebuilds every workspace member.
- **Clippy.** `cargo clippy` uses its own workspace wrapper, so clippy runs
  are not remapped.
- **`profile.trim-paths`** adds `--remap-path-prefix` rules with absolute
  paths, which rustc tracks. Ferry does not rewrite them, so trim-paths and
  remap-paths together are expected to lose the incremental reuse.
- **Not tested:** `cargo doc` and rustdoc, `--timings`, build scripts that
  parse rustc's arguments, proc macros that read paths, coverage tools.
- **Relative paths in output.** rustc's artifact notifications carry the
  rewritten relative paths. `cargo build`, `cargo test` and `cargo run` work
  with them.
- **Debuggers.** Debug info records `.` as the compilation directory, so run
  `gdb` or `lldb` from the worktree root, or map the path in the debugger.
- **Paths captured by code** such as `env!("CARGO_MANIFEST_DIR")` keep the
  real absolute path. As in the normal no-namespace mode, every local crate
  rebuilds once after spawn; with remapping those rebuilds reuse the base's
  incremental state.
- **`.ferry-target`.** Ferry refuses to build if the source root contains a
  real file or directory with that name.
- The approach relies on rustc hashing every path-dependent input into its
  incremental state, the same assumption remapped CI caches make.
