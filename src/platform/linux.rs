//! Linux capability detection: `statfs` magic numbers, sysctls, and the
//! namespace/overlay probe run in a child process.

use std::path::Path;

use super::{Capabilities, FsInfo};

/// Full Linux capability report: kernel release, store filesystem, reflink
/// probe, namespace and overlayfs probes (in a `ferry __probe-sandbox` child
/// process), and the sysctls that explain namespace failures.
pub fn detect(store_root: &Path) -> Capabilities {
    let _ = store_root;
    todo!("platform::linux::detect")
}

/// Filesystem name from `statfs(2)`'s `f_type` magic number.
pub fn fs_info(path: &Path) -> FsInfo {
    let _ = path;
    todo!("platform::linux::fs_info")
}
