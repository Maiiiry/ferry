//! Running commands in an agent's (or base's) build context.
//!
//! With [`Isolation::Namespace`] (Linux), the process enters a private user +
//! mount namespace in which the source tree is bind-mounted at the store's
//! canonical source path and the target dir (a bind mount or an overlayfs)
//! at the canonical target path. Every base and agent of a repository
//! therefore builds at identical absolute paths, which keeps rustc's
//! incremental state reusable. Nothing else in the filesystem changes, so the
//! host paths of the store and worktree stay reachable inside.
//!
//! With [`Isolation::None`], builds run at their host paths with
//! `CARGO_TARGET_DIR` pointing at the agent's own target dir. Crate-level reuse
//! still works; incremental reuse on the first edit does not.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fork::TargetSpec;

#[cfg(target_os = "linux")]
pub mod linux;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    Namespace,
    None,
}

impl std::fmt::Display for Isolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Isolation::Namespace => "namespace",
            Isolation::None => "none",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("namespace sandboxes are not supported on this platform: {0}")]
    Unsupported(String),

    #[error("cannot enter a user namespace from a multi-threaded process")]
    Threaded,

    #[error("sandbox setup failed at {stage}: {source}")]
    Setup {
        stage: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("could not join the running sandbox of pid {pid}: {source}")]
    Join {
        pid: u32,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "the agent's overlay is still mounted by processes in nested namespaces \
         (pids: {pids}) and no process in the agent's own namespace is left to join"
    )]
    OrphanedOverlay { pids: String },

    #[error("path {} cannot be used in an overlayfs mount: {reason}", path.display())]
    UnusablePath { path: PathBuf, reason: &'static str },

    #[error("failed to run `{program}`: {source}")]
    Exec {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

impl SandboxError {
    pub fn hint(&self) -> Option<String> {
        match self {
            SandboxError::Setup { .. } | SandboxError::Unsupported(_) => Some(
                "run `ferry doctor` for details, or set FERRY_SANDBOX=off to build without the canonical-path sandbox"
                    .into(),
            ),
            SandboxError::UnusablePath { .. } => {
                Some("set FERRY_HOME to a path without ',', ':' or '\\'".into())
            }
            SandboxError::OrphanedOverlay { .. } => {
                Some("wait for those processes to exit or kill them, then retry".into())
            }
            _ => None,
        }
    }
}

/// Everything needed to run a command in a build context.
#[derive(Debug, Clone)]
pub struct Plan {
    pub isolation: Isolation,
    /// Host path of the source tree (the worktree).
    pub src: PathBuf,
    pub target: TargetSpec,
    pub canonical_src: PathBuf,
    pub canonical_target: PathBuf,
    /// For overlay targets: where the agent's live namespace is recorded, so
    /// concurrent runs join one mount instead of mounting the same upper dir
    /// twice (which overlayfs does not support).
    pub ns_record: Option<PathBuf>,
}

impl Plan {
    /// The source path builds see.
    pub fn build_src(&self) -> &Path {
        match self.isolation {
            Isolation::Namespace => &self.canonical_src,
            Isolation::None => &self.src,
        }
    }

    /// The target dir builds see (`CARGO_TARGET_DIR`).
    pub fn build_target(&self) -> &Path {
        match (&self.isolation, &self.target) {
            (Isolation::Namespace, _) => &self.canonical_target,
            (Isolation::None, TargetSpec::Dir(dir)) => dir,
            // Unreachable by construction: overlay targets require a namespace.
            (Isolation::None, TargetSpec::Overlay { upper, .. }) => upper,
        }
    }

    /// Maps a working directory into the sandbox: a directory inside the
    /// worktree (by host path, or by canonical path when already inside the
    /// sandbox) maps to the same relative path under [`Plan::build_src`];
    /// anything else maps to the root of the source tree.
    pub fn map_cwd(&self, cwd: &Path) -> PathBuf {
        let resolve = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let cwd = resolve(cwd);
        for root in [&self.canonical_src, &self.src] {
            if let Ok(rel) = cwd.strip_prefix(resolve(root)) {
                return self.build_src().join(rel);
            }
        }
        self.build_src().to_path_buf()
    }
}

/// Result of [`enter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entered {
    /// No isolation; nothing changed.
    Host,
    /// Created a new namespace and mounted the build context.
    Created,
    /// Joined the existing namespace of a running sandbox.
    Joined { pid: u32 },
}

/// Enters the build context in the current process. Irreversible: the
/// process stays in the sandbox for the rest of its life. The process must
/// be single-threaded.
pub fn enter(plan: &Plan) -> Result<Entered, SandboxError> {
    let _ = plan;
    todo!("sandbox::enter")
}

/// PIDs of processes still using the agent sandbox recorded at `ns_record`.
/// Empty when nothing is running or the platform has no namespaces.
pub fn users(ns_record: &Path) -> Result<Vec<u32>, SandboxError> {
    let _ = ns_record;
    todo!("sandbox::users")
}
