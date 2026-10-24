//! Full-copy backend: always available, costs a complete copy per agent.

use super::{Backend, ForkError, ForkPaths, ForkStats, TargetSpec, tree};

#[derive(Debug)]
pub(super) struct Copy;

impl Backend for Copy {
    fn fork(&self, paths: &ForkPaths) -> Result<ForkStats, ForkError> {
        tree::clone_tree(&paths.base_target, &paths.agent_target, tree::copy_file)
    }

    fn target_spec(&self, paths: &ForkPaths) -> TargetSpec {
        TargetSpec::Dir(paths.agent_target.clone())
    }

    fn private_bytes(&self, paths: &ForkPaths) -> Result<u64, ForkError> {
        crate::fsutil::disk_usage(&paths.agent_target).map_err(|e| ForkError::Io {
            op: "measure",
            path: paths.agent_target.clone(),
            source: std::io::Error::other(e.to_string()),
        })
    }
}
