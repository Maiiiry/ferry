# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- The default base also warms `cargo check` and `cargo clippy`:
  `[base] targets` defaults to `["build", "test", "check", "clippy"]`, so an
  agent's first `cargo check` or `cargo clippy` no longer recompiles every
  dependency. The `clippy` target now generates
  `cargo clippy --all-targets --keep-going`, is skipped with a warning when
  clippy is not installed, and only warns when it fails. Existing stores
  build one cold base. Set `targets = ["build", "test"]` for the old
  behaviour.
- Warm commands run with `CARGO_BUILD_WARNINGS=warn`, so `build.warnings =
  "deny"` cannot fail a base.
- `ferry --json base` reports `command_secs` (seconds per warm command) and
  `incomplete` (clippy commands that failed). `ferry doctor` reports whether
  clippy is installed.
