//! Forking a base's target dir into an agent's private build state.
//!
//! INVARIANT: no backend ever writes to the base. Each backend gives the
//! agent state that is physically separate (reflink and copy create new
//! inodes; overlayfs keeps the base as a read-only lower layer).
//!
//! There is deliberately no hardlink backend: Cargo rewrites some files in
//! place (fingerprints are written with `File::create`, which truncates the
//! existing inode), so a hardlinked fork would let an agent's rebuild modify
//! the base.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::BackendPref;
use crate::platform::Capabilities;
use crate::sandbox::Isolation;

#[cfg(target_os = "macos")]
mod clonefile;
mod copy;
#[cfg(target_os = "linux")]
mod overlay;
#[cfg(target_os = "linux")]
mod reflink;
pub(crate) mod tree;

#[derive(Debug, thiserror::Error)]
pub enum ForkError {
    #[error("the {backend} backend is not available here: {reason}")]
    Unavailable {
        backend: BackendKind,
        reason: String,
    },

    #[error("failed to {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl ForkError {
    pub fn hint(&self) -> Option<String> {
        match self {
            ForkError::Unavailable { .. } => Some(
                "run `ferry doctor`, or set FERRY_BACKEND=auto to pick a supported backend".into(),
            ),
            ForkError::Io { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// Copy-on-write clone: `FICLONE` per file on Linux (btrfs, XFS, ...),
    /// `clonefile(2)` of the whole tree on macOS (APFS).
    Reflink,
    /// overlayfs with the base as lower layer and a per-agent upper layer.
    /// Linux only; mounted inside the agent's namespace sandbox.
    Overlay,
    /// Plain recursive copy preserving mtimes. Always available, costs a full
    /// copy of the target dir per agent.
    Copy,
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BackendKind::Reflink => "reflink",
            BackendKind::Overlay => "overlay",
            BackendKind::Copy => "copy",
        })
    }
}

/// Where an agent's build state lives on the host.
#[derive(Debug, Clone)]
pub struct ForkPaths {
    /// The base's target dir. Read-only for every backend.
    pub base_target: PathBuf,
    /// Agent target dir (reflink and copy backends).
    pub agent_target: PathBuf,
    /// overlayfs upper and work dirs (overlay backend).
    pub upper: PathBuf,
    pub work: PathBuf,
}

/// What the sandbox must present to the build as its target dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSpec {
    /// A plain directory, used directly or bind-mounted.
    Dir(PathBuf),
    /// An overlayfs mount; only possible inside a namespace sandbox.
    Overlay {
        lower: PathBuf,
        upper: PathBuf,
        work: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ForkStats {
    pub files: u64,
    pub bytes: u64,
    #[serde(with = "duration_secs")]
    pub elapsed: Duration,
}

pub trait Backend: std::fmt::Debug {
    fn kind(&self) -> BackendKind;

    /// Creates the agent's build state from `paths.base_target`. Must not
    /// modify anything under `base_target`, and must preserve mtimes, since
    /// Cargo's freshness check depends on them.
    fn fork(&self, paths: &ForkPaths) -> Result<ForkStats, ForkError>;

    /// The target dir the sandbox should present to builds.
    fn target_spec(&self, paths: &ForkPaths) -> TargetSpec;

    /// Bytes this agent's build state occupies beyond the base (best
    /// effort: for reflink clones this counts files rewritten since the
    /// fork, because shared extents are not visible through `stat`).
    fn private_bytes(&self, paths: &ForkPaths) -> Result<u64, ForkError>;
}

/// The backend implementation for `kind` on this platform.
pub fn backend(kind: BackendKind) -> Result<Box<dyn Backend>, ForkError> {
    let _ = kind;
    todo!("fork::backend")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub kind: BackendKind,
    /// Why this backend was chosen, for `ferry spawn` and `ferry doctor`.
    pub reason: String,
}

/// Chooses a backend. An explicit preference that cannot be honoured is an
/// error, never a silent fallback.
///
/// `auto` order: reflink when the store's filesystem supports it; overlay
/// when the agent runs in a namespace sandbox and overlayfs works there;
/// otherwise copy.
pub fn select(
    pref: BackendPref,
    caps: &Capabilities,
    isolation: Isolation,
) -> Result<Choice, ForkError> {
    let _ = (pref, caps, isolation);
    todo!("fork::select")
}

/// Removes everything a fork created under the agent dir.
pub fn remove(paths: &ForkPaths) -> crate::Result<()> {
    for dir in [&paths.agent_target, &paths.upper, &paths.work] {
        crate::fsutil::remove_dir_all_force(dir)?;
    }
    Ok(())
}

pub(crate) fn io_err<'a>(
    op: &'static str,
    path: &'a Path,
) -> impl FnOnce(std::io::Error) -> ForkError + 'a {
    move |source| ForkError::Io {
        op,
        path: path.to_path_buf(),
        source,
    }
}

mod duration_secs {
    use std::time::Duration;

    pub fn serialize<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(d.as_secs_f64())
    }
}
