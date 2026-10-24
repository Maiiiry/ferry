//! Linux namespace sandbox: unprivileged user + mount namespace, canonical
//! bind mounts, overlayfs, and joining a running sandbox.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::platform::Probe;

/// Output of `ferry __probe-sandbox`, parsed by the parent process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub userns: Probe,
    pub overlay: Probe,
}

/// Runs in the throwaway `ferry __probe-sandbox <dir>` child: enters a user +
/// mount namespace, bind-mounts and mounts an overlay using scratch dirs
/// under `scratch`, and reports which step failed.
pub fn probe_in_child(scratch: &Path) -> ProbeResult {
    let _ = scratch;
    todo!("sandbox::linux::probe_in_child")
}
