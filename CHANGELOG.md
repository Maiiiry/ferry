# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `ferry base` now seeds a refresh from the previous base even when manifests,
  lockfiles, Cargo configs or toolchain files changed, instead of building
  cold. The cargo shim observes the warm commands' cargo calls, and the units
  they neither compiled nor found fresh are deleted before the base is
  published; when Ferry cannot see every call, the refresh is rebuilt from
  scratch. `[base] prune = false` (or `FERRY_BASE_PRUNE=0`) restores cold
  refreshes. `base.json` and `ferry --json base` gain `observed` and `pruned`.
- `ferry base --watch [--rev REF] [--interval SECS] [--gc]` keeps running and
  builds a base each time the revision settles on a new commit, skipping
  rebases, merges and bisects; `--json` prints one event per line. Spawning an
  agent at another commit than its base now says how to catch up.
