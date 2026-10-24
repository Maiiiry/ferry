//! Detecting what the running system can do: filesystem type, reflinks,
//! unprivileged user namespaces, overlayfs inside them, and the knobs that
//! restrict them (Ubuntu's AppArmor userns restriction, Debian's sysctl).
//!
//! Namespace probes run in a child process (`ferry __probe-sandbox`), because
//! entering a user namespace is irreversible for the calling process.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Macos,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else {
            Os::Linux
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsInfo {
    /// Lowercase filesystem name, e.g. `btrfs`, `xfs`, `ext4`, `tmpfs`,
    /// `apfs`, `zfs`, `overlay`, `nfs`, or `unknown(0x...)`.
    pub name: String,
}

/// Outcome of one capability probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Probe {
    Ok,
    /// Not applicable on this platform (e.g. namespaces on macOS).
    Unsupported {
        reason: String,
    },
    /// Supported in principle but failed here; `stage` names the step
    /// (e.g. "unshare", "uid_map", "mount overlay").
    Failed {
        stage: String,
        error: String,
    },
}

impl Probe {
    pub fn is_ok(&self) -> bool {
        matches!(self, Probe::Ok)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub os: Os,
    /// `uname -r` (Linux) or the Darwin release (macOS).
    pub kernel: String,
    /// Filesystem holding the store (bases and agent build state).
    pub store_fs: FsInfo,
    /// Copy-on-write clones between two files in the store.
    pub reflink: Probe,
    /// Unprivileged user + mount namespace with a self-mapped uid, and a
    /// bind mount inside it.
    pub userns: Probe,
    /// overlayfs mounted inside such a namespace, with upper/work in the store.
    pub overlay: Probe,
    /// Linux sysctls that commonly explain a failed `userns` probe.
    pub sysctls: Vec<Sysctl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sysctl {
    pub name: String,
    pub value: String,
}

impl Capabilities {
    /// Probes everything. `store_root` must exist; probes create and remove
    /// scratch files under it.
    pub fn detect(store_root: &Path) -> Self {
        let _ = store_root;
        todo!("platform::Capabilities::detect")
    }

    pub fn sysctl(&self, name: &str) -> Option<&str> {
        self.sysctls
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.value.as_str())
    }
}

/// Filesystem type of `path`.
pub fn fs_info(path: &Path) -> FsInfo {
    let _ = path;
    todo!("platform::fs_info")
}

/// Tries a copy-on-write clone between two scratch files in `dir`.
pub fn probe_reflink(dir: &Path) -> Probe {
    let _ = dir;
    todo!("platform::probe_reflink")
}
