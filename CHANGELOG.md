# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `backend = "auto"` prefers reflink over overlay where the store supports
  copy-on-write clones (btrfs, XFS, OpenZFS with block cloning), so builds no
  longer pay the overlay mount and upper-filesystem sync on every call.
  Overlay is still picked on filesystems without clones, such as ext4.
  Existing agents keep their backend.
