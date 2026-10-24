# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `ferry setup apparmor` sets up Ubuntu 24.04 and later for the namespace
  sandbox: it installs ferry to `/usr/local/bin` and loads its AppArmor
  profile with one `sudo` call, after printing every step and asking.
  `--uninstall` undoes it.
- `ferry --json doctor` reports the namespace probe's AppArmor label
  (`capabilities.probe_label`), and `ferry doctor` warns when the store is on
  overlayfs and gives container-specific advice inside containers.

### Fixed

- The AppArmor profile lets ferry keep its capabilities when it re-runs
  itself for the namespace probe, which otherwise could report no namespaces
  with the profile loaded. Its file is now named after the binary's path
  (`/etc/apparmor.d/usr.local.bin.ferry`); `ferry setup apparmor` replaces a
  profile installed as `/etc/apparmor.d/ferry`.
