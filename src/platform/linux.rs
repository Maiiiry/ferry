//! Linux capability detection: `statfs` magic numbers, sysctls, and the
//! namespace/overlay probe run in a child process.

use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use super::{Capabilities, FsInfo, Os, Probe, Sysctl};
use crate::sandbox::linux::ProbeResult;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

const SYSCTLS: [&str; 5] = [
    "user.max_user_namespaces",
    "kernel.unprivileged_userns_clone",
    "kernel.apparmor_restrict_unprivileged_userns",
    "kernel.apparmor_restrict_unprivileged_unconfined",
    "kernel.yama.ptrace_scope",
];

/// Full Linux capability report: kernel release, store filesystem, reflink
/// probe, namespace and overlayfs probes (in a `ferry __probe-sandbox` child
/// process), and the sysctls that explain namespace failures.
pub fn detect(store_root: &Path) -> Capabilities {
    let namespaces = probe_namespaces(store_root);
    Capabilities {
        os: Os::Linux,
        kernel: rustix::system::uname()
            .release()
            .to_string_lossy()
            .into_owned(),
        store_fs: fs_info(store_root),
        reflink: super::probe_reflink(store_root),
        userns: namespaces.userns,
        overlay: namespaces.overlay,
        sysctls: sysctls(),
    }
}

/// Filesystem name from `statfs(2)`'s `f_type` magic number.
pub fn fs_info(path: &Path) -> FsInfo {
    let Ok(stat) = rustix::fs::statfs(path) else {
        return FsInfo {
            name: "unknown".into(),
        };
    };
    // A 32-bit magic number in a field whose width and signedness vary by
    // architecture.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let magic = stat.f_type as u32;
    FsInfo {
        name: fs_name(magic),
    }
}

fn fs_name(magic: u32) -> String {
    let name = match magic {
        0x9123_683e => "btrfs",
        0x5846_5342 => "xfs",
        0xef53 => "ext4",
        0x0102_1994 => "tmpfs",
        0x794c_7630 => "overlay",
        0x2fc1_2fc1 => "zfs",
        0xca45_1a4e => "bcachefs",
        0x6969 => "nfs",
        0x6573_5546 => "fuse",
        0xff53_4d42 => "cifs",
        0xfe53_4d42 => "smb2",
        0xf2f5_2010 => "f2fs",
        0xf15f => "ecryptfs",
        0x7371_7368 => "squashfs",
        0x8584_58f6 => "ramfs",
        0x9fa0 => "proc",
        other => return format!("unknown({other:#x})"),
    };
    name.into()
}

/// Runs the namespace probes in a `ferry __probe-sandbox` child, because
/// entering a namespace is irreversible for the calling process.
fn probe_namespaces(store_root: &Path) -> ProbeResult {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let scratch = store_root.join(format!(
        ".ferry-ns-probe-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = fs::create_dir(&scratch)
        .map_err(|e| Probe::Failed {
            stage: "create scratch dir".into(),
            error: e.to_string(),
        })
        .and_then(|()| run_probe(&scratch));
    let _ = crate::fsutil::remove_dir_all_force(&scratch);
    result.unwrap_or_else(|failed| ProbeResult {
        userns: failed.clone(),
        overlay: failed,
    })
}

fn run_probe(scratch: &Path) -> Result<ProbeResult, Probe> {
    let failed = |error: String| Probe::Failed {
        stage: "probe process".into(),
        error,
    };
    let exe = std::env::current_exe().map_err(|e| failed(e.to_string()))?;
    // Files instead of pipes: nothing has to drain them while waiting.
    let (out, err) = (scratch.join("probe.out"), scratch.join("probe.err"));
    let output = |path: &Path| {
        File::create(path)
            .map(Stdio::from)
            .map_err(|e| failed(e.to_string()))
    };
    let mut child = Command::new(&exe)
        .arg("__probe-sandbox")
        .arg(scratch)
        .stdin(Stdio::null())
        .stdout(output(&out)?)
        .stderr(output(&err)?)
        .spawn()
        .map_err(|e| failed(format!("{}: {e}", exe.display())))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed(format!(
                    "no answer within {}s",
                    PROBE_TIMEOUT.as_secs()
                )));
            }
            Err(e) => return Err(failed(e.to_string())),
        }
    };
    serde_json::from_slice(&fs::read(&out).unwrap_or_default()).map_err(|_| {
        let stderr = fs::read_to_string(&err).unwrap_or_default();
        failed(format!(
            "`{} __probe-sandbox` {status} without a result{}",
            exe.display(),
            stderr
                .lines()
                .next()
                .map(|line| format!(": {line}"))
                .unwrap_or_default()
        ))
    })
}

fn sysctls() -> Vec<Sysctl> {
    SYSCTLS
        .iter()
        .filter_map(|name| {
            let value = fs::read_to_string(Path::new("/proc/sys").join(name.replace('.', "/")));
            Some(Sysctl {
                name: (*name).into(),
                value: value.ok()?.trim().into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystems_are_named_by_magic() {
        assert_eq!(fs_name(0x9123_683e), "btrfs");
        assert_eq!(fs_name(0xef53), "ext4");
        assert_eq!(fs_name(0x1234), "unknown(0x1234)");
        assert_eq!(fs_info(Path::new("/proc")).name, "proc");
        assert_eq!(fs_info(Path::new("/nonexistent/ferry")).name, "unknown");
    }

    #[test]
    fn sysctls_are_read_from_proc() {
        let max = sysctls()
            .into_iter()
            .find(|s| s.name == "user.max_user_namespaces");
        if Path::new("/proc/sys/user/max_user_namespaces").exists() {
            assert!(max.is_some_and(|s| s.value.parse::<u64>().is_ok()));
        }
    }

    #[test]
    fn probe_from_a_non_ferry_binary_fails_cleanly() {
        // current_exe is this test binary, which does not answer the probe.
        let dir = tempfile::tempdir().unwrap();
        let caps = detect(dir.path());
        assert!(
            matches!(&caps.userns, Probe::Failed { stage, .. } if stage == "probe process"),
            "{:?}",
            caps.userns
        );
        assert_eq!(caps.userns, caps.overlay);
        assert_ne!(caps.kernel, "");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
