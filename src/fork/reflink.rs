//! Linux reflink backend: `FICLONE` per file (btrfs, XFS, bcachefs,
//! `OpenZFS` 2.2+).

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

use super::{Backend, ForkError, ForkPaths, ForkStats, TargetSpec, tree};

#[derive(Debug)]
pub(super) struct Reflink;

impl Backend for Reflink {
    fn fork(&self, paths: &ForkPaths) -> Result<ForkStats, ForkError> {
        tree::clone_tree(&paths.base_target, &paths.agent_target, clone_file)
    }

    fn target_spec(&self, paths: &ForkPaths) -> TargetSpec {
        TargetSpec::Dir(paths.agent_target.clone())
    }

    fn private_bytes(&self, paths: &ForkPaths) -> Result<u64, ForkError> {
        tree::private_bytes_vs_base(&paths.agent_target, &paths.base_target)
    }
}

/// Creates `dst` sharing all of `src`'s extents. Never falls back to copying:
/// the backend was chosen because clones work, and a silent copy would
/// multiply disk use without anyone noticing.
pub(crate) fn clone_file(src: &Path, dst: &Path) -> io::Result<()> {
    let from = File::open(src)?;
    let to = OpenOptions::new().write(true).create_new(true).open(dst)?;
    rustix::fs::ioctl_ficlone(&to, &from).map_err(io::Error::from)
}
