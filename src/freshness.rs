//! Making a fresh checkout's unchanged files look as old as the base they
//! fork from, without ever hiding a real change from Cargo.
//!
//! Cargo decides freshness by comparing source mtimes with its fingerprint
//! files. A new worktree gets "now" as the mtime of every file, so without
//! intervention every crate looks dirty. Ferry sets ("stamps") the mtime of a
//! file to [`STAMP_SECS`] — far older than any base artifact — but only when
//! that file is proven byte-identical to the file the base compiled. Any file
//! that differs keeps its real (new) mtime, so Cargo rebuilds what depends on
//! it.
//!
//! INVARIANT: a file is stamped only if all of the following hold:
//! 1. it is a tracked regular file at merge stage 0 in the agent's index,
//! 2. the base commit's tree has the same path with the same mode and blob id,
//! 3. the agent's working file and the base checkout's working file have
//!    identical bytes (this catches smudge filters, `core.autocrlf`,
//!    post-checkout hooks, and anything else that makes the working tree
//!    differ from the blob), and
//! 4. its metadata has not changed between verification and stamping.
//!
//! A directory (any ancestor of a tracked path, up to and including the
//! worktree root) is stamped only if its entry names equal those of the same
//! directory in the base checkout. Cargo's `rerun-if-changed=<dir>` takes the
//! recursive max mtime of the directory, its subdirectories and files, and a
//! deleted file is only visible as its parent directory's mtime change.
//!
//! [`StampPlan`] can only be constructed by [`plan`], so code elsewhere cannot
//! stamp an unverified path. Stamping happens once, when an agent is
//! spawned: re-stamping a tree an agent has worked in would hide deletions.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::FreshnessPref;
use crate::error::Result;
use crate::git::Entry;
use crate::toolchain::Toolchain;

/// The mtime given to stamped files: 2023-11-14T22:13:20Z. Any fixed time
/// earlier than every base build works; a fixed value keeps stamping
/// deterministic across bases and agents.
pub const STAMP_SECS: u64 = 1_700_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FreshnessMode {
    /// Cargo compares content hashes (`-Zchecksum-freshness`). Stamping still
    /// runs, for inputs Cargo keeps checking by mtime.
    Checksum,
    /// Cargo compares mtimes; stamping is what makes reuse possible.
    Mtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    /// The path does not exist in the base commit.
    NotInBase,
    /// Same path, different git mode (e.g. became executable or a symlink).
    ModeDiffers,
    /// Same path, different blob.
    BlobDiffers,
    /// The index has the path at a non-zero merge stage.
    Unmerged,
    /// Symlinks and submodules are never stamped.
    NotRegularFile,
    /// Blob ids match but the working-tree bytes differ from the base's.
    ContentDiffers,
    /// The file is missing from the agent or the base working tree.
    Missing,
    /// The file changed between verification and stamping.
    ChangedDuringStamp,
}

/// Files proven identical to the base, ready to stamp.
#[derive(Debug)]
pub struct StampPlan {
    root: PathBuf,
    identical: Vec<Verified>,
    skipped: Vec<(PathBuf, SkipReason)>,
    tracked: usize,
}

/// A verified file plus the metadata observed during verification, so
/// [`apply`] can detect a change in between.
#[derive(Debug)]
#[allow(dead_code)]
struct Verified {
    path: PathBuf,
    len: u64,
    mtime_nsec: i128,
    ino: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StampReport {
    pub tracked: usize,
    pub stamped: usize,
    pub skipped: usize,
}

impl StampPlan {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn identical(&self) -> impl Iterator<Item = &Path> {
        self.identical.iter().map(|v| v.path.as_path())
    }

    pub fn skipped(&self) -> &[(PathBuf, SkipReason)] {
        &self.skipped
    }

    pub fn tracked(&self) -> usize {
        self.tracked
    }
}

/// Decides which of the agent's tracked files may be stamped.
///
/// `base_tree` is `git ls-tree -r` of the base commit, `base_src` the base's
/// checkout, `agent_index` is `git ls-files -s` of the agent worktree at
/// `agent_root`. Comparing file contents may use multiple threads.
pub fn plan(
    base_tree: &[Entry],
    base_src: &Path,
    agent_index: &[Entry],
    agent_root: &Path,
) -> Result<StampPlan> {
    let _ = (base_tree, base_src, agent_index, agent_root);
    todo!("freshness::plan")
}

/// Stamps every file in `plan` whose metadata still matches what was
/// verified. Symlinks are never followed.
pub fn apply(plan: &StampPlan) -> Result<StampReport> {
    let _ = plan;
    todo!("freshness::apply")
}

/// Stamps every tracked regular file of a freshly created base checkout, and
/// every ancestor directory of a tracked path.
///
/// The base checkout is the reference everything else is compared against,
/// so it needs no comparison. Must only be called before the base's first
/// build. Single-threaded: the caller enters a user namespace next, which
/// requires that no other threads exist.
pub fn stamp_base_checkout(base_src: &Path, index: &[Entry]) -> Result<usize> {
    let _ = (base_src, index);
    todo!("freshness::stamp_base_checkout")
}

/// Picks the freshness mode for a new base.
pub fn resolve_mode(pref: FreshnessPref, toolchain: &Toolchain) -> Result<FreshnessMode> {
    let _ = (pref, toolchain);
    todo!("freshness::resolve_mode")
}

/// Environment variables every build of a base (and its agents) must see for
/// `mode`.
pub fn cargo_env(mode: FreshnessMode) -> &'static [(&'static str, &'static str)] {
    let _ = mode;
    todo!("freshness::cargo_env")
}

/// Forces Cargo to rebuild units of a forked target dir that captured
/// absolute paths of the build they were forked from.
///
/// Only needed when an agent builds at different paths than its base (no
/// namespace sandbox). rustc records `# env-dep:CARGO_MANIFEST_DIR=...`,
/// `OUT_DIR` and some `include_str!` paths as absolute paths in its dep-info,
/// and Cargo does not re-check them: such a unit would stay "fresh" while its
/// artifact points at the base's files (a test reading fixtures via
/// `env!("CARGO_MANIFEST_DIR")` would read the base's copies). This scans the
/// rustc dep-info files in `target` (both Cargo build-dir layouts) for any
/// path under `old_roots` and deletes those units' fingerprint directories.
/// Returns the invalidated units' fingerprint directory names.
pub fn invalidate_relocated_units(target: &Path, old_roots: &[PathBuf]) -> Result<Vec<String>> {
    let _ = (target, old_roots);
    todo!("freshness::invalidate_relocated_units")
}
