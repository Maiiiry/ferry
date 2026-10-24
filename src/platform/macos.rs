//! macOS capability detection: `statfs` filesystem names and `clonefile`
//! support.

use std::path::Path;

use super::{Capabilities, FsInfo};

/// macOS capability report: Darwin release, store filesystem, and the
/// `clonefile` probe. Namespaces and overlayfs are reported unsupported.
pub fn detect(store_root: &Path) -> Capabilities {
    let _ = store_root;
    todo!("platform::macos::detect")
}

/// Filesystem name from `statfs(2)`'s `f_fstypename` (e.g. "apfs").
pub fn fs_info(path: &Path) -> FsInfo {
    let _ = path;
    todo!("platform::macos::fs_info")
}
