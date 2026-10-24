//! Small filesystem helpers shared across modules.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, IoContext, Result};

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).at("read", path)?;
    serde_json::from_slice(&bytes).map_err(|source| Error::Metadata {
        path: path.to_path_buf(),
        source,
    })
}

/// Writes `value` so that readers see either the old or the new file, never a
/// torn one. Metadata files double as "ready" markers, so a partial write
/// would make a half-built base look complete.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| Error::Metadata {
        path: path.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    {
        let mut file = fs::File::create(&tmp).at("create", &tmp)?;
        file.write_all(bytes).at("write", &tmp)?;
        file.sync_all().at("sync", &tmp)?;
    }
    fs::rename(&tmp, path).at("rename into place", path)
}

/// Allocated bytes under `path`, counting each hard-linked inode once.
pub fn disk_usage(path: &Path) -> Result<u64> {
    let mut seen = HashSet::new();
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let meta = match fs::symlink_metadata(&dir) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).at("stat", &dir),
        };
        if seen.insert((meta.dev(), meta.ino())) {
            total += meta.blocks() * 512;
        }
        if !meta.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&dir).at("read directory", &dir)? {
            let entry = entry.at("read directory", &dir)?;
            stack.push(entry.path());
        }
    }
    Ok(total)
}

/// `remove_dir_all` that also handles directories without write or search
/// permission. overlayfs leaves a mode-000 `work/work` directory behind, and
/// a plain recursive delete fails on it with EACCES.
pub fn remove_dir_all_force(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {}
    }
    make_dirs_writable(path)?;
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).at("remove", path),
        _ => Ok(()),
    }
}

fn make_dirs_writable(path: &Path) -> Result<()> {
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            fs::set_permissions(&dir, fs::Permissions::from_mode(mode | 0o700))
                .at("chmod", &dir)?;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            stack.push(entry.path());
        }
    }
    Ok(())
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        write_json_atomic(&path, &vec![1, 2, 3]).unwrap();
        write_json_atomic(&path, &vec![4]).unwrap();
        let back: Vec<i32> = read_json(&path).unwrap();
        assert_eq!(back, vec![4]);
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }

    #[test]
    fn force_remove_handles_unsearchable_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent");
        let locked = root.join("work/work");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("f"), b"x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        remove_dir_all_force(&root).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn human_bytes_formats() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(190 * 1024 * 1024), "190.0 MiB");
    }
}
