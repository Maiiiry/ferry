//! The `cargo` shim.
//!
//! `<ferry home>/bin/cargo` is a symlink to the ferry binary. When ferry
//! runs under the name `cargo` it resolves the agent from the working
//! directory on every call and runs the real cargo inside that agent's
//! sandbox, with that agent's target dir. Outside any agent worktree it runs
//! the real cargo untouched. Putting the shim first on `PATH` is therefore
//! safe for every process an agent tool starts, including sub-agents working
//! in other worktrees, which is not true of exporting `CARGO_TARGET_DIR`.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::context::Context;
use crate::error::{Error, IoContext, Result};

pub const NAME: &str = "cargo";

/// Counts shim hops so a misconfigured `PATH` fails instead of looping.
pub(crate) const DEPTH_ENV: &str = "FERRY_SHIM_DEPTH";

/// The hop count to pass to the cargo this shim execs.
pub(crate) fn next_depth() -> String {
    let depth: u32 = std::env::var(DEPTH_ENV)
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0);
    (depth + 1).to_string()
}

/// True if the process was started through the shim symlink.
pub fn invoked_as_cargo() -> bool {
    std::env::args_os()
        .next()
        .and_then(|a| Path::new(&a).file_name().map(|n| n == NAME))
        .unwrap_or(false)
}

/// The directory holding the shim, creating it and (re)pointing the `cargo`
/// symlink at the running ferry binary.
pub fn ensure() -> Result<PathBuf> {
    ensure_link(NAME)
}

/// `$FERRY_HOME/bin`, or `bin` in the platform cache dir. Deliberately not
/// taken from a repository's `ferry.toml`: this directory goes first on
/// agents' `PATH`, so repository content must not choose it.
pub fn bin_dir() -> Result<PathBuf> {
    let home = std::env::var_os("FERRY_HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .or_else(crate::store::default_store_parent)
        .ok_or(Error::NoStoreDir)?;
    crate::fsutil::private_dir(&home)?;
    crate::fsutil::private_dir(&home.join("bin"))
}

/// [`bin_dir`], creating it and (re)pointing the `name` symlink at the
/// running ferry binary.
pub(crate) fn ensure_link(name: &str) -> Result<PathBuf> {
    let dir = bin_dir()?;
    let exe = std::env::current_exe().at("locate", "the ferry binary")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let link = dir.join(name);
    if std::fs::symlink_metadata(&link).is_ok_and(|m| !m.is_symlink()) {
        return Err(Error::Refused(format!(
            "{} exists and is not Ferry's symlink; remove it to install the shim",
            link.display()
        )));
    }
    symlink_atomic(&exe, &link)?;
    Ok(dir)
}

/// Points the symlink `link` at `target`, replacing an existing one
/// atomically so a concurrent reader never finds it missing.
pub(crate) fn symlink_atomic(target: &Path, link: &Path) -> Result<()> {
    if std::fs::read_link(link).ok().as_deref() == Some(target) {
        return Ok(());
    }
    let name = link.file_name().unwrap_or_default().to_string_lossy();
    let tmp = link.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).at("create symlink", &tmp)?;
    std::fs::rename(&tmp, link).at("rename into place", link)
}

/// `PATH` with `shim_dir` first and any other occurrence removed.
pub fn path_with(shim_dir: &Path) -> OsString {
    let mut dirs = vec![shim_dir.to_path_buf()];
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path).filter(|d| !same_dir(d, shim_dir)));
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| shim_dir.as_os_str().to_owned())
}

/// The first `cargo` on `PATH` that is not a ferry binary: neither this one
/// nor another ferry's shim (two shims picking each other would loop).
pub fn real_cargo() -> Option<PathBuf> {
    let me = std::env::current_exe()
        .ok()
        .and_then(|e| std::fs::metadata(e).ok());
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(NAME))
        .find(|candidate| {
            let is_ferry = std::fs::canonicalize(candidate)
                .is_ok_and(|real| real.file_name().is_some_and(|n| n == "ferry"));
            match std::fs::metadata(candidate) {
                Ok(meta) => {
                    meta.is_file()
                        && meta.mode() & 0o111 != 0
                        && !is_ferry
                        && !me
                            .as_ref()
                            .is_some_and(|m| m.dev() == meta.dev() && m.ino() == meta.ino())
                }
                Err(_) => false,
            }
        })
}

/// Entry point when invoked as `cargo`.
pub fn main() -> ExitCode {
    let depth: u32 = std::env::var(DEPTH_ENV)
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(0);
    if depth >= 4 {
        warn("cargo resolves back to the shim; check PATH for several Ferry installations");
        return ExitCode::FAILURE;
    }
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(cargo) = real_cargo() else {
        warn("no real `cargo` found on PATH after the shim");
        return ExitCode::from(127);
    };
    let err = match run(&cargo, &args) {
        Ok(never) => match never {},
        Err(e) => e,
    };
    warn(&err.to_string());
    if let Some(hint) = err.hint() {
        let _ = writeln!(std::io::stderr(), "  hint: {hint}");
    }
    ExitCode::FAILURE
}

fn warn(message: &str) {
    let _ = writeln!(std::io::stderr(), "ferry (cargo shim): {message}");
}

fn run(cargo: &Path, args: &[OsString]) -> Result<std::convert::Infallible> {
    let cwd = std::env::current_dir().at("read", "current directory")?;
    let agent = match Context::discover_existing(&cwd) {
        Ok(Some(ctx)) => {
            crate::agent::find_by_root(&ctx, &ctx.repo.toplevel)?.map(|meta| (ctx, meta))
        }
        // No store yet, or not a repository Ferry can manage: plain cargo is
        // exactly right.
        Ok(None) | Err(Error::Git(_)) => None,
        Err(e) => {
            // Plain cargo is still safe here: it builds into the checkout's
            // own target dir, never into another agent's.
            warn(&format!("{e}; running cargo without Ferry"));
            None
        }
    };
    match agent {
        Some((ctx, meta)) => crate::agent::exec_cargo(&ctx, &meta, cargo, args, &cwd),
        None => Err(exec_plain(cargo, args)),
    }
}

fn exec_plain(cargo: &Path, args: &[OsString]) -> Error {
    let source = std::process::Command::new(cargo)
        .args(args)
        .env(DEPTH_ENV, next_depth())
        .exec();
    Error::Sandbox(crate::sandbox::SandboxError::Exec {
        program: cargo.display().to_string(),
        source,
    })
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let resolve = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    resolve(a) == resolve(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_dir_resolves_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(same_dir(&real, &link));
        assert!(!same_dir(&real, dir.path()));
    }
}
