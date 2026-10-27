# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3](https://github.com/patwrall/ferry/compare/v0.1.2...v0.1.3) - 2026-10-27

### Other

- publish rust-analyzer, uv, ruff and polars benchmarks
- *(demo)* re-record without recorder overhead or Cargo noise
- shorten the README tagline
- rewrite the README as a short front page
- add a recorded demo of a cold worktree against a Ferry agent

## [0.1.2](https://github.com/patwrall/ferry/compare/v0.1.1...v0.1.2) - 2026-10-26

### Fixed

- *(base)* version the unit-inputs hash
- *(agent)* keep incremental builds when CI is set
- *(base)* count incremental settings among unit inputs
- *(freshness)* rebuild units that read through a retargeted symlink under checksum freshness

### Other

- Merge pull request #14 from patwrall/fix/symlink-checksum-freshness
- *(shim)* check build.incremental in Cargo home's config
- *(freshness)* say which units a retargeted symlink invalidates

## [0.1.1](https://github.com/patwrall/ferry/compare/v0.1.0...v0.1.1) - 2026-10-24

### Other

- restore the Homebrew install command

## [0.1.0](https://github.com/patwrall/ferry/releases/tag/v0.1.0) - 2026-10-24

The first release.

### Added

- `ferry base` builds one immutable, warm Cargo build of a commit; a refresh
  for a newer commit is seeded from the previous base, and `--watch` rebuilds
  it whenever the watched revision moves. Bases warm `build`, `test`, `check`
  and `clippy` by default.
- `ferry spawn` gives an agent its own git worktree and a fork of the base's
  target dir: a reflink clone (btrfs, XFS, APFS), an overlayfs layer, or a
  copy. `auto` prefers reflink.
- On Linux, agents build in a user and mount namespace where every agent's
  worktree and target dir appear at the same two paths, so the first edit
  compiles incrementally. Elsewhere an agent reuses every unchanged crate
  except the few that captured the base's paths, and edited crates recompile
  from scratch.
- `ferry run`, `ferry shell`, `ferry env` and a cargo shim (`ferry shim`)
  route every cargo call in an agent's worktree to its own target dir,
  including nested workspaces.
- `ferry list`, `ferry rm` and `ferry gc` manage agents and bases;
  `ferry doctor` reports what the machine supports and what Ferry will do.
- `ferry hook` for Claude Code's `WorktreeCreate`, `WorktreeRemove` and
  `SessionStart` hooks, and a launcher example for Codex.
- `ferry setup apparmor` installs Ferry and the AppArmor profile Ubuntu 24.04
  and later need for the namespace sandbox, with one `sudo` call.
- Experimental remap-paths mode (`[sandbox] remap-paths = true` or
  `FERRY_REMAP_PATHS=1`, rustc 1.98 or newer) keeps most of a library's
  incremental state across paths without namespaces.
