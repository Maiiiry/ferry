//! Advisory `flock(2)` locks on files in the store.
//!
//! Locks guard Ferry's own metadata (agent registry, base construction,
//! namespace join/create). They are never held while an agent's build runs.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

use crate::error::{IoContext, Result};

#[derive(Debug)]
pub struct FileLock {
    _file: File,
    path: PathBuf,
}

impl FileLock {
    /// Blocks until the exclusive lock is acquired. `what` names the resource
    /// in the message printed while waiting.
    pub fn exclusive(path: &Path, what: &str) -> Result<Self> {
        Self::acquire(
            path,
            what,
            FlockOperation::NonBlockingLockExclusive,
            FlockOperation::LockExclusive,
        )
    }

    pub fn shared(path: &Path, what: &str) -> Result<Self> {
        Self::acquire(
            path,
            what,
            FlockOperation::NonBlockingLockShared,
            FlockOperation::LockShared,
        )
    }

    /// Returns `None` if another process holds the lock.
    pub fn try_exclusive(path: &Path) -> Result<Option<Self>> {
        let file = open_lock_file(path)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Some(Self {
                _file: file,
                path: path.to_path_buf(),
            })),
            Err(Errno::WOULDBLOCK) => Ok(None),
            Err(e) => Err(std::io::Error::from(e)).at("lock", path),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn acquire(
        path: &Path,
        what: &str,
        try_op: FlockOperation,
        op: FlockOperation,
    ) -> Result<Self> {
        let file = open_lock_file(path)?;
        match flock(&file, try_op) {
            Ok(()) => {}
            Err(Errno::WOULDBLOCK) => {
                eprintln!("ferry: waiting for another ferry process to release {what}");
                flock(&file, op)
                    .map_err(std::io::Error::from)
                    .at("lock", path)?;
            }
            Err(e) => return Err(std::io::Error::from(e)).at("lock", path),
        }
        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }
}

fn open_lock_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .at("open lock file", path)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Retries `check` for up to two seconds. Other test threads fork
    /// children, and until such a child execs it shares this process's open
    /// file descriptions, so a dropped lock can stay held for a moment.
    pub(crate) fn eventually(mut check: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !check() {
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        true
    }

    #[test]
    fn exclusive_excludes_and_releases_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let held = FileLock::exclusive(&path, "test").unwrap();
        assert!(FileLock::try_exclusive(&path).unwrap().is_none());
        drop(held);
        assert!(eventually(|| FileLock::try_exclusive(&path)
            .unwrap()
            .is_some()));
    }
}
