//! The `cargo` shim.
//!
//! `<store parent>/bin/cargo` is a symlink to the ferry binary. When ferry
//! runs under the name `cargo` it resolves the agent from the working
//! directory on every call and runs the real cargo inside that agent's
//! sandbox, with that agent's target dir. Outside any agent worktree it runs
//! the real cargo untouched. Putting the shim first on `PATH` is therefore
//! safe for every process an agent tool starts, including sub-agents working
//! in other worktrees, which is not true of exporting `CARGO_TARGET_DIR`.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::context::Context;
use crate::error::{Error, IoContext, Result};

pub const NAME: &str = "cargo";

/// True if the process was started through the shim symlink.
pub fn invoked_as_cargo() -> bool {
    std::env::args_os()
        .next()
        .and_then(|a| Path::new(&a).file_name().map(|n| n == NAME))
        .unwrap_or(false)
}

/// `<store parent>/bin`, creating it and (re)pointing the `cargo` symlink at
/// the running ferry binary.
pub fn ensure(ctx: &Context) -> Result<PathBuf> {
    ensure_link(ctx, NAME)
}

/// `<store parent>/bin`, creating it and (re)pointing the `name` symlink at
/// the running ferry binary.
pub(crate) fn ensure_link(ctx: &Context, name: &str) -> Result<PathBuf> {
    let parent = ctx.settings.store_parent.clone().ok_or(Error::NoStoreDir)?;
    let dir = parent.join("bin");
    std::fs::create_dir_all(&dir).at("create directory", &dir)?;
    let exe = std::env::current_exe().at("locate", "the ferry binary")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    symlink_atomic(&exe, &dir.join(name))?;
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

/// The first `cargo` on `PATH` that is not this ferry binary.
pub fn real_cargo() -> Option<PathBuf> {
    let me = std::env::current_exe()
        .ok()
        .and_then(|e| std::fs::metadata(e).ok());
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(NAME))
        .find(|candidate| match std::fs::metadata(candidate) {
            Ok(meta) => {
                meta.is_file()
                    && meta.mode() & 0o111 != 0
                    && !me
                        .as_ref()
                        .is_some_and(|m| m.dev() == meta.dev() && m.ino() == meta.ino())
            }
            Err(_) => false,
        })
}

/// Entry point when invoked as `cargo`.
pub fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(cargo) = real_cargo() else {
        eprintln!("ferry (cargo shim): no real `cargo` found on PATH after the shim");
        return ExitCode::from(127);
    };
    let err = match run(&cargo, &args) {
        Ok(never) => match never {},
        Err(e) => e,
    };
    eprintln!("ferry (cargo shim): {err}");
    if let Some(hint) = err.hint() {
        eprintln!("  hint: {hint}");
    }
    ExitCode::FAILURE
}

fn run(cargo: &Path, args: &[OsString]) -> Result<std::convert::Infallible> {
    let cwd = std::env::current_dir().at("read", "current directory")?;
    let agent = match Context::discover(&cwd) {
        Ok(ctx) => crate::agent::find_by_dir(&ctx, &cwd)?.map(|meta| (ctx, meta)),
        // Not a repository Ferry can manage: plain cargo is exactly right.
        Err(Error::Git(_)) => None,
        Err(e) => {
            // Plain cargo is still safe here: it builds into the checkout's
            // own target dir, never into another agent's.
            eprintln!("ferry (cargo shim): {e}; running cargo without Ferry");
            None
        }
    };
    match agent {
        Some((ctx, meta)) => crate::agent::exec_cargo(&ctx, &meta, cargo, args, &cwd),
        None => Err(exec_plain(cargo, args)),
    }
}

fn exec_plain(cargo: &Path, args: &[OsString]) -> Error {
    let source = std::process::Command::new(cargo).args(args).exec();
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
