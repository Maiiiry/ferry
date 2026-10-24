//! macOS capability detection: `statfs` filesystem names and `clonefile`
//! support.

use std::path::Path;

use super::{Capabilities, FsInfo, Os, Probe};

/// macOS capability report: Darwin release, store filesystem, and the
/// `clonefile` probe. Namespaces and overlayfs are reported unsupported.
pub fn detect(store_root: &Path) -> Capabilities {
    let no_namespaces = || Probe::Unsupported {
        reason: "macOS has no mount namespaces".into(),
    };
    Capabilities {
        os: Os::Macos,
        kernel: rustix::system::uname()
            .release()
            .to_string_lossy()
            .into_owned(),
        store_fs: fs_info(store_root),
        reflink: super::probe_reflink(store_root),
        userns: no_namespaces(),
        overlay: no_namespaces(),
        sysctls: Vec::new(),
        probe_label: None,
    }
}

/// Filesystem name from `statfs(2)`'s `f_fstypename` (e.g. "apfs").
pub fn fs_info(path: &Path) -> FsInfo {
    let name = match rustix::fs::statfs(path) {
        Ok(fs) => {
            let bytes: Vec<u8> = fs
                .f_fstypename
                .iter()
                .take_while(|&&c| c != 0)
                .map(|&c| u8::from_ne_bytes(c.to_ne_bytes()))
                .collect();
            String::from_utf8_lossy(&bytes).to_ascii_lowercase()
        }
        Err(_) => "unknown".into(),
    };
    FsInfo { name }
}

/// Sets the attribute behind `NSURLIsExcludedFromBackupKey`, which Cargo sets
/// on its target dirs (through Core Foundation). Its value is a binary
/// property list holding the string `com.apple.backupd`.
pub fn exclude_from_backups(path: &Path) -> std::io::Result<()> {
    const NAME: &str = "com.apple.metadata:com_apple_backup_excludeItem";
    const VALUE: &[u8] = b"bplist00_\x10\x11com.apple.backupd\x08\
        \0\0\0\0\0\0\x01\x01\0\0\0\0\0\0\0\x01\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x1c";
    rustix::fs::setxattr(path, NAME, VALUE, rustix::fs::XattrFlags::empty())
        .map_err(std::io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_store_filesystem_and_no_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let caps = detect(dir.path());
        assert_eq!(caps.os, Os::Macos);
        assert!(
            caps.kernel.starts_with(|c: char| c.is_ascii_digit()),
            "{}",
            caps.kernel
        );
        assert_ne!(caps.store_fs.name, "unknown");
        assert!(matches!(caps.userns, Probe::Unsupported { .. }));
        assert!(matches!(caps.overlay, Probe::Unsupported { .. }));
        if caps.store_fs.name == "apfs" {
            assert_eq!(caps.reflink, Probe::Ok);
        }
        let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 0, "reflink probe left files behind");
    }

    #[test]
    fn backup_exclusion_is_recognized_by_time_machine() {
        let dir = tempfile::tempdir().unwrap();
        exclude_from_backups(dir.path()).unwrap();
        let mut value = [0u8; 128];
        let len = rustix::fs::getxattr(
            dir.path(),
            "com.apple.metadata:com_apple_backup_excludeItem",
            &mut value,
        )
        .unwrap();
        assert!(value[..len].starts_with(b"bplist00"));
        match std::process::Command::new("tmutil")
            .arg("isexcluded")
            .arg(dir.path())
            .output()
        {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                assert!(stdout.contains("[Excluded]"), "tmutil said: {stdout}");
            }
            other => eprintln!("skipping the tmutil check: {other:?}"),
        }
    }
}
