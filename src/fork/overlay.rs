//! Linux overlayfs backend: the base target is the read-only lower layer,
//! each agent gets its own upper and work dirs. The mount itself happens in
//! the agent's namespace sandbox (see `sandbox::linux`).

use super::{Backend, ForkError, ForkPaths, ForkStats, TargetSpec, io_err};

#[derive(Debug)]
pub(super) struct Overlay;

impl Backend for Overlay {
    fn fork(&self, paths: &ForkPaths) -> Result<ForkStats, ForkError> {
        let started = std::time::Instant::now();
        for dir in [&paths.upper, &paths.work] {
            std::fs::create_dir_all(dir).map_err(io_err("create directory", dir))?;
        }
        Ok(ForkStats {
            elapsed: started.elapsed(),
            ..ForkStats::default()
        })
    }

    fn target_spec(&self, paths: &ForkPaths) -> TargetSpec {
        TargetSpec::Overlay {
            lower: paths.base_target.clone(),
            upper: paths.upper.clone(),
            work: paths.work.clone(),
        }
    }

    fn private_bytes(&self, paths: &ForkPaths) -> Result<u64, ForkError> {
        crate::fsutil::disk_usage(&paths.upper).map_err(|e| ForkError::Io {
            op: "measure",
            path: paths.upper.clone(),
            source: std::io::Error::other(e.to_string()),
        })
    }
}
