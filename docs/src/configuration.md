# Configuration

Ferry works without any configuration. To change what it does, use a
`ferry.toml` file, `FERRY_*` environment variables, or both.

## Where the file is read from

1. `$FERRY_CONFIG`, if set. The file must exist.
2. Otherwise `ferry.toml` in the root of the repository's **main checkout**, if
   it exists. Ferry reads the file from the working tree, so it does not need
   to be committed, and agents' worktrees do not need their own copy.
3. Otherwise the built-in defaults.

`ferry init` writes the commented default file; `ferry doctor` shows which
file was loaded. Unknown keys and invalid values are errors, so a typo cannot
silently fall back to a default.

A `ferry.toml` in a repository you clone can run arbitrary commands when you
run `ferry base`, like a `Makefile` or a `build.rs`.

## Precedence

Lowest to highest:

1. built-in defaults;
2. the top-level tables of `ferry.toml`;
3. its `[linux]` or `[macos]` table, for the platform Ferry runs on;
4. `FERRY_*` environment variables.

## The default file

This is exactly what `ferry init` writes. Every value in it is the default.

```toml
# Ferry configuration: https://patwrall.github.io/ferry/configuration.html

[base]
# Cargo subcommands that warm the base: "check", "build", "test" (--no-run), "clippy".
targets = ["build", "test"]
# Cargo profiles to warm. "dev" uses Cargo's default profile selection.
profiles = ["dev"]
# Extra arguments for every generated command.
cargo-args = ["--workspace"]
# Replace the generated commands entirely; each entry runs with `sh -c`.
# commands = ["cargo build --workspace --all-targets --locked"]

[agent]
# Branch created for each agent: <prefix><name>.
branch-prefix = "ferry/"
# Where agent worktrees (and their uncommitted work) live, one subdirectory per repository.
# Default: $XDG_DATA_HOME/ferry/worktrees or ~/.local/share/ferry/worktrees (Linux),
# ~/Library/Application Support/ferry/worktrees (macOS).
# worktree-dir = "~/agents"

[store]
# Parent directory for Ferry's per-repository build state (bases, forks).
# Default: $XDG_CACHE_HOME/ferry or ~/.cache/ferry (Linux), ~/Library/Caches/ferry (macOS).
# dir = "/fast/disk/ferry"

[fork]
# How an agent's target dir is forked from the base: "auto", "reflink", "overlay", "copy".
backend = "auto"

[sandbox]
# "auto" uses a private mount namespace when available (Linux), "namespace" requires it, "off" never uses it.
mode = "auto"
# Experimental: keep rustc's incremental state reusable when building without the
# namespace sandbox (macOS; Linux without user namespaces). Needs rustc 1.98+.
remap-paths = false

[freshness]
# "auto" uses checksum freshness when the toolchain supports it, otherwise mtime stamping.
mode = "auto"

# Per-platform overrides of backend, sandbox and freshness.
# [linux]
# sandbox = "namespace"
# [macos]
# backend = "reflink"
```

## `[base]`: what warms a base

`ferry base` runs the warm commands, in order, at the root of the base's
checkout (at the canonical path, inside the namespace sandbox when it is in
use). Each runs with `sh -c`, with `CARGO_TARGET_DIR` and
`CARGO_BUILD_BUILD_DIR` set to the base's target dir. Their output goes to
stderr. If one fails, the base is discarded.

Warm what your agents will run. Cargo reuses a unit only when the agent's
invocation selects the same package, features, profile and target kind.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `targets` | array of `"check"`, `"build"`, `"test"`, `"clippy"` | `["build", "test"]` | Cargo subcommands to run. `"test"` runs `cargo test --no-run`, which builds the test binaries without running them. |
| `profiles` | array of profile names | `["dev"]` | Profiles to warm. `"dev"` adds no flag (Cargo's own selection, so tests keep using the `test` profile), `"release"` adds `--release`, any other name adds `--profile <name>`. Names must be non-empty and contain no whitespace. |
| `cargo-args` | array of strings | `["--workspace"]` | Appended to every generated command, shell-quoted. |
| `commands` | array of strings | unset | Replaces the generated commands. Each entry is a shell command line. Must not be empty when set. |

Without `commands`, Ferry generates one command per profile and target, in
that order: for each profile, for each target,
`cargo <subcommand> [profile flag] <cargo-args...>`. `targets` and `profiles`
must not be empty then. With the defaults that is:

```sh
cargo build --workspace
cargo test --no-run --workspace
```

The warm commands are part of the base's identity: changing them makes the
next `ferry base` build a new base.

Do not pass `--target-dir` or `--config build.target-dir=...` /
`--config build.build-dir=...` in warm commands. Command-line options beat
the environment variables Ferry sets, so the build would land outside the
base.

## `[agent]`: worktrees and branches

| Key | Type | Default | Meaning |
|---|---|---|---|
| `branch-prefix` | string | `"ferry/"` | Each agent's branch is `<branch-prefix><name>`, unless `ferry spawn --branch` names it. |
| `worktree-dir` | path | see below | Parent directory for agent worktrees. Each repository gets a subdirectory `<repo-name>-<hash>`, and each agent a directory inside it. A leading `~` is expanded; a relative path is relative to the main checkout. |

The worktree parent directory is, in order of precedence:

1. `FERRY_WORKTREES`;
2. `[agent] worktree-dir`;
3. `<store parent>/worktrees`, when the store parent was set explicitly with
   `FERRY_HOME` or `[store] dir`;
4. `$XDG_DATA_HOME/ferry/worktrees` or `~/.local/share/ferry/worktrees` on
   Linux, `~/Library/Application Support/ferry/worktrees` on macOS.

`ferry spawn --path <dir>` overrides all of these for one agent.

## `[store]`: build state

| Key | Type | Default | Meaning |
|---|---|---|---|
| `dir` | path | `$XDG_CACHE_HOME/ferry` or `~/.cache/ferry` (Linux), `~/Library/Caches/ferry` (macOS) | Parent directory for Ferry's build state: one store per repository (bases, forks, canonical mount points) plus `bin/`, which holds the cargo shim. A leading `~` is expanded; a relative path is relative to the main checkout. |

`FERRY_HOME` overrides it. The resolved path must be absolute. Put it on a
local disk, ideally a copy-on-write filesystem (btrfs, XFS): forks then become
reflinks. Network and FUSE filesystems make builds slow
and mtimes unreliable; `ferry doctor` warns about them. On Linux kernels
before 6.13, the overlay backend refuses store paths containing `,`, `:`, `\`
or a newline, which those kernels' overlayfs mount options cannot express.

## `[fork]`: how agents fork the base

| Key | Values | Default |
|---|---|---|
| `backend` | `"auto"`, `"reflink"`, `"overlay"`, `"copy"` | `"auto"` |

- `auto` picks `reflink` when the store's filesystem supports copy-on-write
  clones, else `overlay` when the agent runs in the namespace sandbox and
  overlayfs works there, else `copy`. An agent keeps the backend it was
  spawned with.
- `overlay`: Linux only, and needs the namespace sandbox.
- `reflink`: Linux on btrfs, XFS or OpenZFS 2.2+; macOS on APFS.
- `copy`: a full copy, available everywhere.

An explicit backend that is not available is an error, never a silent
fallback. See [How it works](how-it-works.md#forks) for the trade-offs.

## `[sandbox]`

| Key | Values | Default | Meaning |
|---|---|---|---|
| `mode` | `"auto"`, `"namespace"`, `"off"` | `"auto"` | `auto` uses the Linux namespace sandbox when it works and builds at host paths otherwise. `namespace` requires it: `ferry base` and `ferry spawn` fail without it. `off` never uses it. |
| `remap-paths` | boolean | `false` | Experimental. Path-independent incremental builds for agents that run without the namespace sandbox. Needs rustc 1.98 or newer. See [Path remapping](remap-paths.md). |

The isolation a base was built with is part of its identity. Agents of a base
built with the namespace sandbox use it too; if namespaces stop working later,
such agents build at host paths with a warning (or fail, with
`mode = "namespace"`).

## `[freshness]`

| Key | Values | Default |
|---|---|---|
| `mode` | `"auto"`, `"checksum"`, `"mtime"` | `"auto"` |

- `mtime`: Cargo's normal freshness check, made reliable by stamping files
  that are identical to the base (see
  [How it works](how-it-works.md#stamping-making-the-fork-look-fresh)).
- `checksum`: Cargo's unstable checksum freshness. rustc inputs are compared
  by size and hash instead of mtime; build-script inputs, rustdoc and the
  comparison of a unit's outputs with its dependencies' stay mtime-based, so
  Ferry still stamps. Needs a nightly toolchain; on stable, `checksum` is an
  error. Ferry sets `CARGO_UNSTABLE_CHECKSUM_FRESHNESS=true` and
  `CARGO_BUILD_FINGERPRINT=content` for the base and every agent.
- `auto`: `checksum` on nightly toolchains, `mtime` otherwise.

Switching the mode rebuilds every unit, so it is part of the base's identity.

## `[linux]` and `[macos]`

Per-platform overrides. Only the table for the running platform applies.

| Key | Overrides | Values |
|---|---|---|
| `backend` | `[fork] backend` | `"auto"`, `"reflink"`, `"overlay"`, `"copy"` |
| `sandbox` | `[sandbox] mode` | `"auto"`, `"namespace"`, `"off"` |
| `freshness` | `[freshness] mode` | `"auto"`, `"checksum"`, `"mtime"` |

```toml
[linux]
sandbox = "namespace"

[macos]
backend = "reflink"
```

## Environment variables

Variables Ferry reads. They override `ferry.toml`.

| Variable | Overrides | Values |
|---|---|---|
| `FERRY_HOME` | `[store] dir` | Absolute path of the store parent. Also moves the default worktree parent to `$FERRY_HOME/worktrees`. Empty means unset. |
| `FERRY_CONFIG` | config file location | Path of the config file to load instead of `<main checkout>/ferry.toml`. |
| `FERRY_WORKTREES` | `[agent] worktree-dir` | Absolute path of the worktree parent. Empty means unset. |
| `FERRY_BACKEND` | `backend` | `auto`, `reflink`, `overlay`, `copy` |
| `FERRY_SANDBOX` | `sandbox` mode | `auto`, `namespace`, `off` |
| `FERRY_FRESHNESS` | `freshness` mode | `auto`, `checksum`, `mtime` |
| `FERRY_BASE_COMMAND` | `[base]` warm commands | One shell command that replaces all warm commands. Blank means unset. |
| `FERRY_REMAP_PATHS` | `[sandbox] remap-paths` | `1` or `true`; `0`, `false` or empty |

An invalid value is an error that names the variable and the accepted values.

## Environment Ferry sets

For commands started with `ferry run` and `ferry shell`:

| Variable | Value |
|---|---|
| `PATH` | the shim directory first, then your `PATH` |
| `FERRY_AGENT` | the agent's name, for your scripts and prompts (Ferry never reads it) |
| `FERRY_WORKTREE` | the agent's worktree (host path) |

For cargo started by the shim (and for warm commands):

| Variable | Value |
|---|---|
| `CARGO_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR` | the agent's target dir as the build sees it |
| `CARGO_UNSTABLE_CHECKSUM_FRESHNESS`, `CARGO_BUILD_FINGERPRINT` | `true`, `content` (checksum freshness only) |
| `RUSTC_WORKSPACE_WRAPPER` | the `ferry-rustc` wrapper (remap-paths mode only) |

Ferry deliberately never exports `CARGO_TARGET_DIR` to anything else. See
[Agent integrations](agents.md#the-rules).

## What makes a new base

A base is reused only if its commit and fingerprint match. The fingerprint
covers the `rustc -vV` and `cargo -V` output, the commit's
`rust-toolchain.toml` or `rust-toolchain` file, the warm commands, the
freshness mode, the isolation (namespace sandbox or not) and remap-paths.
Changing any of them and running `ferry base` builds a new base. Existing
agents keep using the base they were forked from.

## Examples

Annotated variants are in
[`examples/config/`](https://github.com/patwrall/ferry/tree/main/examples/config):

- `ci-checks.toml`: warm `check`, `clippy` and `test` for all targets with
  `--locked`.
- `release-profile.toml`: warm the dev and release profiles.
- `custom-commands.toml`: your own warm commands.
- `workspace-in-subdirectory.toml`: a Cargo workspace below the repository
  root (`cd rust && cargo build --workspace`).
- `platform-overrides.toml`: store location, branch prefix and explicit
  per-platform choices.
