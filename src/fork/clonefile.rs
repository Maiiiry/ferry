//! macOS backend: one `clonefile(2)` call clones the whole base target on
//! APFS.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{Backend, BackendKind, ForkError, ForkPaths, ForkStats, TargetSpec, io_err, tree};

// From <sys/clonefile.h>; the libc crate does not define them.
const CLONE_NOFOLLOW: u32 = 0x1;
const CLONE_NOOWNERCOPY: u32 = 0x2;

#[derive(Debug)]
pub(super) struct Clonefile;

impl Backend for Clonefile {
    fn kind(&self) -> BackendKind {
        BackendKind::Reflink
    }

    fn fork(&self, paths: &ForkPaths) -> Result<ForkStats, ForkError> {
        let started = Instant::now();
        let (base, dst) = (paths.base_target.as_path(), paths.agent_target.as_path());
        if fs::symlink_metadata(dst).is_ok() {
            return Err(io_err("create", dst)(io::ErrorKind::AlreadyExists.into()));
        }
        let mut staging = dst.as_os_str().to_owned();
        staging.push(format!(".tmp-{}", std::process::id()));
        let staging = PathBuf::from(staging);
        let _ = fs::remove_dir_all(&staging);
        let result = clone_dir(base, &staging).and_then(|stats| {
            fs::rename(&staging, dst).map_err(io_err("rename into place", dst))?;
            Ok(stats)
        });
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        Ok(ForkStats {
            elapsed: started.elapsed(),
            ..result?
        })
    }

    fn target_spec(&self, paths: &ForkPaths) -> TargetSpec {
        TargetSpec::Dir(paths.agent_target.clone())
    }

    fn private_bytes(&self, paths: &ForkPaths) -> Result<u64, ForkError> {
        tree::private_bytes_vs_base(&paths.agent_target, &paths.base_target)
    }
}

/// Clones the tree at `base` into `dst`, which must not exist.
///
/// Apple discourages cloning directories because the source hierarchy is
/// locked against changes until the clone finishes. A base is immutable, so
/// nothing ever waits on that lock. Hard links inside the tree may come out
/// as independent clones; Cargo re-creates its links when it relinks.
fn clone_dir(base: &Path, dst: &Path) -> Result<ForkStats, ForkError> {
    match clonefile(base, dst, CLONE_NOOWNERCOPY) {
        Ok(()) => restore_metadata(base, dst),
        Err(e)
            if matches!(
                e.raw_os_error(),
                Some(libc::ENOTSUP | libc::EXDEV | libc::EINVAL)
            ) =>
        {
            let _ = fs::remove_dir_all(dst);
            tree::clone_tree(base, dst, clone_file)
        }
        Err(e) => Err(io_err("clone", base)(e)),
    }
}

/// The man page only promises the clone "identical" attributes, and Cargo's
/// freshness check depends on mtimes, so this compares every entry with the
/// base and restores times and permissions that differ.
fn restore_metadata(base: &Path, clone: &Path) -> Result<ForkStats, ForkError> {
    let mut stats = ForkStats::default();
    let mut stack = vec![(base.to_path_buf(), clone.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        let want = fs::symlink_metadata(&from).map_err(io_err("stat", &from))?;
        if want.is_dir() {
            for entry in fs::read_dir(&to).map_err(io_err("read directory", &to))? {
                let name = entry.map_err(io_err("read directory", &to))?.file_name();
                stack.push((from.join(&name), to.join(&name)));
            }
        } else if want.is_file() {
            stats.files += 1;
            stats.bytes += want.len();
        }
        // After reading the directory, which may have updated its atime.
        restore(&to, &want)?;
    }
    Ok(stats)
}

fn restore(path: &Path, want: &fs::Metadata) -> Result<(), ForkError> {
    let have = fs::symlink_metadata(path).map_err(io_err("stat", path))?;
    let symlink = want.file_type().is_symlink();
    let mode = want.mode() & 0o7777;
    if !symlink && have.mode() & 0o7777 != mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(io_err("chmod", path))?;
    }
    let times = |m: &fs::Metadata| (m.mtime(), m.mtime_nsec(), m.atime(), m.atime_nsec());
    if times(&have) != times(want) {
        tree::set_times(path, want, symlink).map_err(io_err("set times on", path))?;
    }
    Ok(())
}

/// Clones one regular file with `clonefile(2)`. Used by the capability probe
/// and per file when a directory clone is not possible. Never falls back to
/// copying, for the same reason as the Linux reflink backend.
pub(crate) fn clone_file(src: &Path, dst: &Path) -> io::Result<()> {
    clonefile(src, dst, CLONE_NOFOLLOW | CLONE_NOOWNERCOPY)
}

fn clonefile(src: &Path, dst: &Path, flags: u32) -> io::Result<()> {
    let src = CString::new(src.as_os_str().as_bytes())?;
    let dst = CString::new(dst.as_os_str().as_bytes())?;
    // SAFETY: both pointers come from live `CString`s, so they are valid and
    // NUL-terminated for the duration of the call.
    if unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    const OLD: Duration = Duration::from_secs(1_600_000_000);

    fn fixture(root: &Path) {
        fs::create_dir_all(root.join("debug/deps")).unwrap();
        fs::create_dir_all(root.join("debug/.fingerprint/a-1")).unwrap();
        fs::write(root.join("debug/deps/liba-1.rlib"), b"rlib").unwrap();
        fs::hard_link(
            root.join("debug/deps/liba-1.rlib"),
            root.join("debug/liba.rlib"),
        )
        .unwrap();
        fs::write(root.join("debug/.fingerprint/a-1/lib-a"), b"fp").unwrap();
        fs::write(root.join("debug/app"), b"#!bin").unwrap();
        fs::set_permissions(root.join("debug/app"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("deps/liba-1.rlib", root.join("debug/link")).unwrap();
        fs::File::options()
            .write(true)
            .open(root.join("debug/app"))
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + OLD)
            .unwrap();
    }

    fn paths(dir: &Path) -> ForkPaths {
        ForkPaths {
            base_target: dir.join("base"),
            agent_target: dir.join("agent/target"),
            upper: dir.join("agent/upper"),
            work: dir.join("agent/work"),
        }
    }

    fn mtime(path: &Path) -> SystemTime {
        fs::symlink_metadata(path).unwrap().modified().unwrap()
    }

    fn clones_supported(dir: &Path) -> bool {
        let probe = crate::platform::probe_reflink(dir);
        if !probe.is_ok() {
            eprintln!("skipping: no clonefile on {}: {probe:?}", dir.display());
        }
        probe.is_ok()
    }

    #[test]
    fn fork_clones_contents_modes_and_mtimes() {
        let dir = tempfile::tempdir().unwrap();
        if !clones_supported(dir.path()) {
            return;
        }
        let p = paths(dir.path());
        fixture(&p.base_target);
        fs::create_dir_all(dir.path().join("agent")).unwrap();

        let stats = Clonefile.fork(&p).unwrap();
        assert_eq!(stats.files, 4);
        let (base, fork) = (&p.base_target, &p.agent_target);
        assert_eq!(
            fs::read(fork.join("debug/.fingerprint/a-1/lib-a")).unwrap(),
            b"fp"
        );
        let app = fs::metadata(fork.join("debug/app")).unwrap();
        assert_eq!(app.permissions().mode() & 0o777, 0o755);
        assert_eq!(app.modified().unwrap(), SystemTime::UNIX_EPOCH + OLD);
        for rel in ["debug", "debug/deps", "debug/liba.rlib", "debug/link"] {
            assert_eq!(mtime(&fork.join(rel)), mtime(&base.join(rel)), "{rel}");
        }
        assert_eq!(
            fs::read_link(fork.join("debug/link")).unwrap(),
            PathBuf::from("deps/liba-1.rlib")
        );
        let staging_left = fs::read_dir(dir.path().join("agent"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().contains(".tmp-"));
        assert!(!staging_left, "staging directory left behind");

        // What Cargo does to a fingerprint: truncate and rewrite in place.
        fs::write(fork.join("debug/.fingerprint/a-1/lib-a"), b"changed").unwrap();
        fs::write(fork.join("debug/liba.rlib"), b"changed").unwrap();
        assert_eq!(
            fs::read(base.join("debug/.fingerprint/a-1/lib-a")).unwrap(),
            b"fp"
        );
        assert_eq!(
            fs::read(base.join("debug/deps/liba-1.rlib")).unwrap(),
            b"rlib"
        );
        assert!(Clonefile.private_bytes(&p).unwrap() > 0);
    }

    #[test]
    fn fork_refuses_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        fixture(&p.base_target);
        fs::create_dir_all(&p.agent_target).unwrap();
        assert!(Clonefile.fork(&p).is_err());
        assert_eq!(fs::read_dir(&p.agent_target).unwrap().count(), 0);
    }

    #[test]
    fn restore_metadata_fixes_times_and_modes() {
        let dir = tempfile::tempdir().unwrap();
        let (base, copy) = (dir.path().join("base"), dir.path().join("copy"));
        fixture(&base);
        tree::clone_tree(&base, &copy, tree::copy_file).unwrap();
        let app = copy.join("debug/app");
        fs::set_permissions(&app, fs::Permissions::from_mode(0o600)).unwrap();
        fs::File::options()
            .write(true)
            .open(&app)
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();

        let stats = restore_metadata(&base, &copy).unwrap();
        assert_eq!(stats.files, 4);
        let meta = fs::metadata(&app).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o755);
        assert_eq!(meta.modified().unwrap(), SystemTime::UNIX_EPOCH + OLD);
    }

    #[test]
    fn per_file_fallback_clones_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        if !clones_supported(dir.path()) {
            return;
        }
        let (base, fork) = (dir.path().join("base"), dir.path().join("fork"));
        fixture(&base);
        let stats = tree::clone_tree(&base, &fork, clone_file).unwrap();
        assert_eq!(stats.files, 4);
        assert_eq!(fs::read(fork.join("debug/liba.rlib")).unwrap(), b"rlib");
    }
}
