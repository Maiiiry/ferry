//! macOS backend: one `clonefile(2)` call clones the whole base target on
//! APFS.

use super::{Backend, BackendKind, ForkError, ForkPaths, ForkStats, TargetSpec, tree};

#[derive(Debug)]
pub(super) struct Clonefile;

impl Backend for Clonefile {
    fn kind(&self) -> BackendKind {
        BackendKind::Reflink
    }

    fn fork(&self, paths: &ForkPaths) -> Result<ForkStats, ForkError> {
        let _ = paths;
        todo!("fork::clonefile::Clonefile::fork")
    }

    fn target_spec(&self, paths: &ForkPaths) -> TargetSpec {
        TargetSpec::Dir(paths.agent_target.clone())
    }

    fn private_bytes(&self, paths: &ForkPaths) -> Result<u64, ForkError> {
        tree::private_bytes_vs_base(&paths.agent_target, &paths.base_target)
    }
}

/// Clones one regular file with `clonefile(2)`. Used by the capability probe.
pub(crate) fn clone_file(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    let _ = (src, dst);
    todo!("fork::clonefile::clone_file")
}
