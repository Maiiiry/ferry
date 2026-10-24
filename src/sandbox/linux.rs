//! Linux namespace sandbox: unprivileged user + mount namespace, canonical
//! bind mounts, overlayfs, and joining a running sandbox.
//!
//! Two overlay mounts on one upper dir give diverging views, and in a user
//! namespace the kernel only warns about it, so Ferry guarantees one mount per
//! agent: the namespace holding it is recorded in `ns.json`, and every process
//! that can use the mount holds `mount.lock` shared. A new mount is created
//! only when that lock is free and no process is left in the recorded
//! namespace; otherwise the caller joins it.

use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::mem::ManuallyDrop;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fs::{
    AtFlags, CWD, FlockOperation, Mode, OFlags, StatVfsMountFlags, StatxFlags, flock,
};
use rustix::io::Errno;
use rustix::mount::{
    FsMountFlags, FsOpenFlags, MountAttrFlags, MountFlags, MountPropagationFlags, MoveMountFlags,
};
use rustix::thread::{LinkNameSpaceType, UnshareFlags};
use serde::{Deserialize, Serialize};

use super::{Entered, Plan, SandboxError};
use crate::fork::TargetSpec;
use crate::platform::Probe;

/// How long transient states may take to clear: a joined thread still listed
/// in `/proc/self/task`, or `mount.lock` briefly held by [`users`].
const SETTLE: Duration = Duration::from_millis(100);
const POLL: Duration = Duration::from_millis(5);

/// `mount(2)` copies at most one page of options; 4 KiB is the smallest page
/// size Linux runs with.
const LEGACY_OPTIONS_MAX: usize = 4096;

/// Output of `ferry __probe-sandbox`, parsed by the parent process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub userns: Probe,
    pub overlay: Probe,
}

/// Device and inode of a file; for `/proc/<pid>/ns/*` the namespace identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileId {
    dev: u64,
    ino: u64,
}

impl FileId {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }

    fn of_path(path: impl AsRef<Path>) -> io::Result<Self> {
        fs::metadata(path).map(|m| Self::of(&m))
    }
}

/// `ns.json`: the namespaces holding an agent's overlay mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NsRecord {
    /// The process that created them; it may have exited since.
    pid: u32,
    mnt: FileId,
    user: FileId,
    /// The worktree as mounted at `canonical_src` inside the namespace.
    /// Namespace inode numbers are reused once a namespace is gone, so
    /// membership is confirmed by what a process sees at that path.
    canonical_src: PathBuf,
    src: FileId,
}

impl NsRecord {
    fn read(path: &Path) -> Option<Self> {
        crate::fsutil::read_json(path).ok()
    }

    fn is_member(&self, pid: u32) -> bool {
        let proc = PathBuf::from(format!("/proc/{pid}"));
        let in_src = proc.join("root").join(
            self.canonical_src
                .strip_prefix("/")
                .unwrap_or(&self.canonical_src),
        );
        FileId::of_path(proc.join("ns/mnt")).is_ok_and(|id| id == self.mnt)
            && FileId::of_path(in_src).is_ok_and(|id| id == self.src)
    }

    /// Live processes in the recorded mount namespace, the creator first.
    fn members(&self) -> impl Iterator<Item = u32> + '_ {
        let others = fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter(|&pid| pid != self.pid);
        std::iter::once(self.pid)
            .chain(others)
            .filter(|&pid| self.is_member(pid))
    }
}

/// Enters `plan`'s sandbox in the calling process (see [`super::enter`]).
///
/// Bind-mount-only targets get a fresh namespace every time. For overlay
/// targets the caller must hold the agent lock, which serializes creators.
pub fn enter(plan: &Plan) -> Result<Entered, SandboxError> {
    ensure_single_threaded()?;
    match (&plan.target, &plan.ns_record) {
        (TargetSpec::Dir(_), _) => create(plan).map(|()| Entered::Created),
        (TargetSpec::Overlay { .. }, Some(record)) => enter_overlay(plan, record),
        (TargetSpec::Overlay { .. }, None) => Err(SandboxError::Setup {
            stage: "mount overlay",
            source: io::Error::new(
                io::ErrorKind::InvalidInput,
                "an overlay target needs a namespace record",
            ),
        }),
    }
}

/// PIDs of processes using the overlay sandbox recorded at `ns_record`: the
/// live members of its mount namespace and, while `mount.lock` is held, the
/// lock's live holders. Empty when there is no `mount.lock` (bind-only
/// agents, or never entered).
///
/// When the lock is held but no holder can be attributed (they are in other
/// pid namespaces, or exited leaving descendants in nested namespaces), this
/// returns the recorded pid even if it is stale, so callers treat the agent
/// as busy.
pub fn users(ns_record: &Path) -> Result<Vec<u32>, SandboxError> {
    let lock_path = mount_lock_path(ns_record);
    let held = match File::open(&lock_path) {
        Ok(lock) => match flock(&lock, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => false,
            Err(Errno::WOULDBLOCK) => true,
            Err(e) => return Err(failed("mount lock", None)(e)),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(failed("mount lock", None)(e)),
    };
    let record = NsRecord::read(ns_record);
    let pids = in_use_by(record.as_ref(), &lock_path, held);
    if held && pids.is_empty() {
        return match record {
            Some(record) => Ok(vec![record.pid]),
            None => Err(SandboxError::OrphanedOverlay {
                pids: join_pids(&[]),
            }),
        };
    }
    Ok(pids)
}

/// Runs in the throwaway `ferry __probe-sandbox <dir>` child: enters a user +
/// mount namespace, bind-mounts and mounts an overlay using scratch dirs
/// under `scratch`, and reports which step failed.
pub fn probe_in_child(scratch: &Path) -> ProbeResult {
    match probe(scratch) {
        Ok(()) => ProbeResult {
            userns: Probe::Ok,
            overlay: Probe::Ok,
        },
        Err((stage, error)) => {
            let failed = Probe::Failed {
                stage: stage.into(),
                error,
            };
            let userns = if stage == "mount overlay" || stage.starts_with("overlay ") {
                Probe::Ok
            } else {
                failed.clone()
            };
            ProbeResult {
                userns,
                overlay: failed,
            }
        }
    }
}

fn probe(scratch: &Path) -> Result<(), (&'static str, String)> {
    let path = |rel: &str| scratch.join(rel);
    let prepare = |e: io::Error| ("prepare scratch dir", e.to_string());
    for dir in ["src", "lower/sub", "upper", "work", "mnt-src", "mnt-target"] {
        fs::create_dir_all(path(dir)).map_err(prepare)?;
    }
    fs::write(path("lower/sub/f"), b"ferry").map_err(prepare)?;
    let plan = Plan {
        isolation: super::Isolation::Namespace,
        src: path("src"),
        target: TargetSpec::Overlay {
            lower: path("lower"),
            upper: path("upper"),
            work: path("work"),
        },
        canonical_src: path("mnt-src"),
        canonical_target: path("mnt-target"),
        ns_record: None,
        remap_wrapper: None,
    };
    ensure_single_threaded()
        .and_then(|()| create(&plan))
        .map_err(|e| match e {
            SandboxError::Setup { stage, source } => (stage, source.to_string()),
            SandboxError::UnusablePath { .. } => ("mount overlay", e.to_string()),
            other => ("unshare", other.to_string()),
        })?;
    let merged = path("mnt-target");
    fs::write(merged.join("written"), b"ferry").map_err(|e| ("overlay write", e.to_string()))?;
    fs::remove_file(merged.join("sub/f"))
        .and_then(|()| fs::remove_dir(merged.join("sub")))
        .map_err(|e| ("overlay rmdir", e.to_string()))
}

/// `unshare(CLONE_NEWUSER)` and `setns` into a user namespace fail with
/// EINVAL in a multi-threaded process.
fn ensure_single_threaded() -> Result<(), SandboxError> {
    let deadline = Instant::now() + SETTLE;
    loop {
        let threads = fs::read_dir("/proc/self/task")
            .map_err(failed("unshare", None))?
            .count();
        if threads <= 1 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(SandboxError::Threaded);
        }
        std::thread::sleep(POLL);
    }
}

fn enter_overlay(plan: &Plan, record_path: &Path) -> Result<Entered, SandboxError> {
    let lock_path = mount_lock_path(record_path);
    // Never closed and deliberately not O_CLOEXEC: the command this process
    // execs and all of its descendants inherit the descriptor, so the lock is
    // held exactly as long as any process may still use the mount.
    let lock = ManuallyDrop::new(
        rustix::fs::open(
            &lock_path,
            OFlags::RDWR | OFlags::CREATE,
            Mode::from_raw_mode(0o644),
        )
        .map_err(failed("mount lock", None))?,
    );
    let deadline = Instant::now() + SETTLE;
    loop {
        let exclusive = match flock(&*lock, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => true,
            Err(Errno::WOULDBLOCK) => false,
            Err(e) => return Err(failed("mount lock", None)(e)),
        };
        let record = NsRecord::read(record_path);
        // Members are joined even when the lock is free: a process that
        // closed its inherited descriptors can still be using the mount.
        if let Some(record) = &record
            && let Some(pid) = join_member(record)?
        {
            lock_shared(&lock)?;
            return Ok(Entered::Joined { pid });
        }
        if exclusive {
            create(plan)?;
            let record = current_record(plan).map_err(failed("namespace record", None))?;
            crate::fsutil::write_json_atomic(record_path, &record)
                .map_err(|e| failed("namespace record", None)(io::Error::other(e)))?;
            lock_shared(&lock)?;
            return Ok(Entered::Created);
        }
        if Instant::now() >= deadline {
            return Err(SandboxError::OrphanedOverlay {
                pids: join_pids(&in_use_by(record.as_ref(), &lock_path, true)),
            });
        }
        std::thread::sleep(POLL);
    }
}

/// Downgrading is not atomic (flock(2)), but every process that could mount
/// is serialized by the agent lock, and [`users`] still finds this process
/// through the namespace scan.
fn lock_shared(lock: &OwnedFd) -> Result<(), SandboxError> {
    flock(lock, FlockOperation::LockShared).map_err(failed("mount lock", None))
}

fn current_record(plan: &Plan) -> io::Result<NsRecord> {
    Ok(NsRecord {
        pid: std::process::id(),
        mnt: FileId::of_path("/proc/self/ns/mnt")?,
        user: FileId::of_path("/proc/self/ns/user")?,
        canonical_src: plan.canonical_src.clone(),
        src: FileId::of_path(&plan.canonical_src)?,
    })
}

/// Joins the user and mount namespaces of a live member of `record`;
/// returns the member's pid, or `None` if there is no joinable member.
fn join_member(record: &NsRecord) -> Result<Option<u32>, SandboxError> {
    for pid in record.members() {
        // The descriptors pin the namespaces, so what is checked here is what
        // gets joined even if `pid` exits or is reused in between.
        let open = |kind: &str, expected: FileId| {
            let file = File::open(format!("/proc/{pid}/ns/{kind}")).ok()?;
            (FileId::of(&file.metadata().ok()?) == expected).then_some(file)
        };
        // A member whose user namespace differs is nested and cannot be joined.
        let (Some(user), Some(mnt)) = (open("user", record.user), open("mnt", record.mnt)) else {
            continue;
        };
        rustix::thread::move_into_link_name_space(user.as_fd(), Some(LinkNameSpaceType::User))
            .and_then(|()| {
                rustix::thread::move_into_link_name_space(
                    mnt.as_fd(),
                    Some(LinkNameSpaceType::Mount),
                )
            })
            .map_err(|e| SandboxError::Join {
                pid,
                source: e.into(),
            })?;
        return Ok(Some(pid));
    }
    Ok(None)
}

/// Creates a new user + mount namespace for the calling process and mounts
/// `plan`'s source and target at their canonical paths.
fn create(plan: &Plan) -> Result<(), SandboxError> {
    // After unshare these read as the overflow id until the maps are written.
    let uid = rustix::process::getuid().as_raw();
    let gid = rustix::process::getgid().as_raw();
    // SAFETY: the caller checked that the process is single-threaded, and
    // without CLONE_FILES no file descriptor table is split.
    unsafe { rustix::thread::unshare_unsafe(UnshareFlags::NEWUSER | UnshareFlags::NEWNS) }
        .map_err(|e| failed("unshare", unshare_note(e))(e))?;
    let note = apparmor_note();
    write_proc("/proc/self/setgroups", "deny").map_err(failed("setgroups", note))?;
    write_proc("/proc/self/uid_map", &format!("{uid} {uid} 1\n"))
        .map_err(failed("uid_map", note))?;
    write_proc("/proc/self/gid_map", &format!("{gid} {gid} 1\n"))
        .map_err(failed("gid_map", note))?;
    rustix::mount::mount_change(
        "/",
        MountPropagationFlags::REC | MountPropagationFlags::PRIVATE,
    )
    .map_err(failed("mount propagation", note))?;
    rustix::mount::mount_bind(&plan.src, &plan.canonical_src)
        .map_err(failed("bind source", note))?;
    match &plan.target {
        TargetSpec::Dir(dir) => rustix::mount::mount_bind(dir, &plan.canonical_target)
            .map_err(failed("bind target", note)),
        TargetSpec::Overlay { lower, upper, work } => {
            mount_overlay(lower, upper, work, &plan.canonical_target, note)
        }
    }
}

/// Mounts overlayfs on `target` with `userxattr`, without which the mount
/// works in a user namespace but removing a lower directory fails with EIO.
fn mount_overlay(
    lower: &Path,
    upper: &Path,
    work: &Path,
    target: &Path,
    note: Option<&'static str>,
) -> Result<(), SandboxError> {
    let fail = |e: io::Error| SandboxError::Setup {
        stage: "mount overlay",
        source: annotate(e, note),
    };
    clear_dir(work).map_err(fail)?;
    match overlay_context(lower, upper, work) {
        Ok(context) => {
            rustix::mount::fsconfig_create(&context)
                .and_then(|()| {
                    rustix::mount::fsmount(
                        &context,
                        FsMountFlags::FSMOUNT_CLOEXEC,
                        MountAttrFlags::empty(),
                    )
                })
                .and_then(|mnt| {
                    rustix::mount::move_mount(
                        &mnt,
                        "",
                        CWD,
                        target,
                        MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
                    )
                })
                .map_err(|e| fail(e.into()))?;
        }
        // EINVAL: overlayfs before 6.13 takes no directory fds. ENOSYS/EPERM:
        // no fsopen(2), or a seccomp filter that predates it.
        Err(Errno::INVAL | Errno::NOSYS | Errno::PERM) => {
            let options = legacy_options(lower, upper, work)?;
            rustix::mount::mount(
                "overlay",
                target,
                "overlay",
                MountFlags::empty(),
                options.as_c_str(),
            )
            .map_err(|e| fail(e.into()))?;
        }
        Err(e) => return Err(fail(e.into())),
    }
    // overlayfs falls back to a read-only mount when it cannot set up its
    // work dir, which would only surface later as failing builds.
    let stat = rustix::fs::statvfs(target).map_err(|e| fail(e.into()))?;
    if stat.f_flag.contains(StatVfsMountFlags::RDONLY) {
        return Err(fail(Errno::ROFS.into()));
    }
    Ok(())
}

fn overlay_context(lower: &Path, upper: &Path, work: &Path) -> rustix::io::Result<OwnedFd> {
    let context = rustix::mount::fsopen("overlay", FsOpenFlags::FSOPEN_CLOEXEC)?;
    rustix::mount::fsconfig_set_flag(&context, "userxattr")?;
    for (key, dir) in [("lowerdir+", lower), ("upperdir", upper), ("workdir", work)] {
        let fd = rustix::fs::open(
            dir,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        rustix::mount::fsconfig_set_fd(&context, key, &fd)?;
    }
    Ok(context)
}

/// Options for `mount(2)`, which splits them on ',' and lowerdir on ':' with
/// '\' escapes; paths that would need escaping are refused instead.
fn legacy_options(lower: &Path, upper: &Path, work: &Path) -> Result<CString, SandboxError> {
    let unusable = |path: &Path, reason| SandboxError::UnusablePath {
        path: path.to_path_buf(),
        reason,
    };
    for path in [lower, upper, work] {
        if path
            .as_os_str()
            .as_bytes()
            .iter()
            .any(|b| matches!(b, b',' | b':' | b'\\' | b'\n'))
        {
            return Err(unusable(
                path,
                "it contains ',', ':', '\\' or a newline, which this kernel's overlayfs mount options cannot express",
            ));
        }
    }
    let options = [
        b"lowerdir=".as_slice(),
        lower.as_os_str().as_bytes(),
        b",upperdir=",
        upper.as_os_str().as_bytes(),
        b",workdir=",
        work.as_os_str().as_bytes(),
        b",userxattr",
    ]
    .concat();
    if options.len() >= LEGACY_OPTIONS_MAX {
        return Err(unusable(
            upper,
            "the overlayfs mount options would be longer than one page",
        ));
    }
    CString::new(options).map_err(|_| unusable(upper, "it contains a NUL byte"))
}

/// Empties overlayfs's work dir: leftovers of an earlier mount can make the
/// next one fail (`work/work/incompat/volatile`) or come up read-only. No
/// other mount of this overlay exists when this runs (see [`enter_overlay`]).
fn clear_dir(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            crate::fsutil::remove_dir_all_force(&entry.path()).map_err(io::Error::other)?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn write_proc(path: &str, contents: &str) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .write_all(contents.as_bytes())
}

fn failed<E: Into<io::Error>>(
    stage: &'static str,
    note: Option<&'static str>,
) -> impl FnOnce(E) -> SandboxError {
    move |e| SandboxError::Setup {
        stage,
        source: annotate(e.into(), note),
    }
}

/// Adds the likely cause to an error, keeping its kind: a bare EPERM does not
/// say which policy to change.
fn annotate(error: io::Error, note: Option<&str>) -> io::Error {
    match note {
        Some(note) => io::Error::new(error.kind(), format!("{error}; {note}")),
        None => error,
    }
}

fn unshare_note(errno: Errno) -> Option<&'static str> {
    match errno {
        Errno::PERM => Some(
            "user namespaces are blocked, usually by a container's seccomp policy or an agent tool's \
             sandbox (Claude Code 2.1.305+ blocks them unless sandbox.allowNestedUserNamespaces is \
             set), or by kernel.unprivileged_userns_clone=0",
        ),
        Errno::NOSPC => {
            Some("user namespaces are disabled or exhausted (user.max_user_namespaces)")
        }
        _ => None,
    }
}

/// Ubuntu 24.04+ lets the unshare succeed but moves the process to a
/// capability-less `unprivileged_userns` profile, so a later step fails
/// instead.
fn apparmor_note() -> Option<&'static str> {
    fs::read_to_string("/proc/self/attr/current")
        .ok()
        .filter(|label| label.starts_with("unprivileged_userns"))
        .map(|_| "Ubuntu AppArmor restriction (label unprivileged_userns)")
}

fn mount_lock_path(ns_record: &Path) -> PathBuf {
    ns_record.with_file_name("mount.lock")
}

/// Members of the recorded namespace, plus the live lock holders if the
/// lock is held; sorted and deduplicated.
fn in_use_by(record: Option<&NsRecord>, lock_path: &Path, held: bool) -> Vec<u32> {
    let mut pids: Vec<u32> = record.into_iter().flat_map(NsRecord::members).collect();
    if held {
        pids.extend(lock_holders(lock_path));
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Live processes that `/proc/locks` lists as holding a `flock` on `path`.
fn lock_holders(path: &Path) -> Vec<u32> {
    let Some((dev, ino)) = lock_key(path) else {
        return Vec::new();
    };
    let Ok(locks) = fs::read_to_string("/proc/locks") else {
        return Vec::new();
    };
    flock_holders(&locks, dev, ino)
        .into_iter()
        .filter(|pid| Path::new(&format!("/proc/{pid}")).exists())
        .collect()
}

/// `/proc/locks` names a file by its superblock's device, which can differ
/// from `st_dev` (btrfs reports a per-subvolume device), so the device is
/// taken from the file's mount in mountinfo.
fn lock_key(path: &Path) -> Option<((u32, u32), u64)> {
    let stx = rustix::fs::statx(
        CWD,
        path,
        AtFlags::empty(),
        StatxFlags::INO | StatxFlags::MNT_ID,
    )
    .ok()?;
    if !StatxFlags::from_bits_retain(stx.stx_mask).contains(StatxFlags::MNT_ID) {
        return None;
    }
    let mountinfo = fs::read_to_string("/proc/self/mountinfo").ok()?;
    Some((mount_dev(&mountinfo, stx.stx_mnt_id)?, stx.stx_ino))
}

/// `major:minor` (decimal) of mount `id` in a mountinfo listing.
fn mount_dev(mountinfo: &str, id: u64) -> Option<(u32, u32)> {
    mountinfo.lines().find_map(|line| {
        let mut fields = line.split(' ');
        if fields.next()?.parse::<u64>().ok()? != id {
            return None;
        }
        let (major, minor) = fields.nth(1)?.split_once(':')?;
        Some((major.parse().ok()?, minor.parse().ok()?))
    })
}

/// PIDs owning `FLOCK` entries for `dev` (hex `major:minor` in `/proc/locks`)
/// and `ino`. Blocked waiters (`->`) are not holders.
fn flock_holders(locks: &str, dev: (u32, u32), ino: u64) -> Vec<u32> {
    locks
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().skip(1);
            if fields.next()? != "FLOCK" {
                return None;
            }
            let pid = fields.nth(2)?.parse().ok()?;
            let mut id = fields.next()?.split(':');
            let major = u32::from_str_radix(id.next()?, 16).ok()?;
            let minor = u32::from_str_radix(id.next()?, 16).ok()?;
            let inode = id.next()?.parse::<u64>().ok()?;
            ((major, minor) == dev && inode == ino).then_some(pid)
        })
        .collect()
}

fn join_pids(pids: &[u32]) -> String {
    if pids.is_empty() {
        return "unknown".into();
    }
    pids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::FileLock;

    #[test]
    fn ns_record_parses() {
        let json = r#"{"pid": 42, "mnt": {"dev": 4, "ino": 4026532871},
            "user": {"dev": 4, "ino": 4026532870},
            "canonical_src": "/s/mnt/src", "src": {"dev": 50, "ino": 256}}"#;
        let record: NsRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.pid, 42);
        assert_eq!(
            record.mnt,
            FileId {
                dev: 4,
                ino: 4_026_532_871
            }
        );
        assert_eq!(record.canonical_src, Path::new("/s/mnt/src"));
        let back: NsRecord =
            serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn flock_holders_match_device_and_inode() {
        let locks = "\
1: POSIX  ADVISORY  WRITE 900 00:20:777 0 EOF
2: FLOCK  ADVISORY  WRITE 901 00:20:777 0 EOF
2: -> FLOCK  ADVISORY  WRITE 902 00:20:777 0 EOF
3: FLOCK  ADVISORY  READ 903 00:20:777 0 EOF
4: FLOCK  ADVISORY  READ 904 00:21:777 0 EOF
5: FLOCK  ADVISORY  READ 905 00:20:7770 0 EOF
6: FLOCK  ADVISORY  WRITE 906 fd:01:12 0 EOF
";
        assert_eq!(flock_holders(locks, (0, 0x20), 777), vec![901, 903]);
        assert_eq!(flock_holders(locks, (0xfd, 1), 12), vec![906]);
        assert_eq!(
            flock_holders("garbage\n\n", (0, 0x20), 777),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn mount_dev_reads_decimal_major_minor() {
        let mountinfo = "\
22 1 0:21 / /proc rw,nosuid shared:5 - proc proc rw
35 1 0:32 /@home /home rw,relatime shared:2 - btrfs /dev/nvme1n1p3 rw
";
        assert_eq!(mount_dev(mountinfo, 35), Some((0, 32)));
        assert_eq!(mount_dev(mountinfo, 3), None);
    }

    #[test]
    fn lock_holders_finds_this_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mount.lock");
        let _held = FileLock::shared(&path, "test").unwrap();
        assert_eq!(lock_holders(&path), vec![std::process::id()]);
    }

    #[test]
    fn users_reports_lock_state() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("ns.json");
        assert_eq!(users(&record).unwrap(), Vec::<u32>::new(), "no lock file");
        let lock = mount_lock_path(&record);
        drop(FileLock::exclusive(&lock, "test").unwrap());
        assert_eq!(users(&record).unwrap(), Vec::<u32>::new(), "free lock");

        let held = FileLock::shared(&lock, "test").unwrap();
        assert_eq!(users(&record).unwrap(), vec![std::process::id()]);
        crate::fsutil::write_json_atomic(
            &record,
            &NsRecord {
                pid: 1,
                mnt: FileId { dev: 0, ino: 1 },
                user: FileId { dev: 0, ino: 1 },
                canonical_src: dir.path().join("mnt/src"),
                src: FileId { dev: 0, ino: 1 },
            },
        )
        .unwrap();
        assert_eq!(users(&record).unwrap(), vec![std::process::id()]);
        drop(held);
        assert_eq!(users(&record).unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn legacy_options_refuse_paths_that_need_escaping() {
        let (lower, upper, work) = (Path::new("/b/t"), Path::new("/a/u"), Path::new("/a/w"));
        assert_eq!(
            legacy_options(lower, upper, work).unwrap().as_bytes(),
            b"lowerdir=/b/t,upperdir=/a/u,workdir=/a/w,userxattr"
        );
        for bad in ["/a,b", "/a:b", "/a\\b", "/a\nb"] {
            assert!(matches!(
                legacy_options(Path::new(bad), upper, work),
                Err(SandboxError::UnusablePath { .. })
            ));
            assert!(legacy_options(lower, upper, Path::new(bad)).is_err());
        }
        let long = PathBuf::from(format!("/{}", "x".repeat(LEGACY_OPTIONS_MAX)));
        assert!(matches!(
            legacy_options(lower, &long, work),
            Err(SandboxError::UnusablePath { .. })
        ));
    }

    #[test]
    fn notes_explain_common_failures() {
        let e = annotate(Errno::PERM.into(), unshare_note(Errno::PERM));
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert!(e.to_string().contains("allowNestedUserNamespaces"), "{e}");
        assert!(unshare_note(Errno::NOSPC).is_some());
        assert!(unshare_note(Errno::INVAL).is_none());
    }
}
