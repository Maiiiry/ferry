//! The `cargo` shim.
//!
//! `<ferry home>/bin/cargo` is a symlink to the ferry binary. When ferry
//! runs under the name `cargo` it resolves the agent from the working
//! directory on every call and runs the real cargo inside that agent's
//! sandbox, with that agent's target dir. Outside any agent worktree it runs
//! the real cargo untouched. Putting the shim first on `PATH` is therefore
//! safe for every process an agent tool starts, including sub-agents working
//! in other worktrees, which is not true of exporting `CARGO_TARGET_DIR`.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

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
    crate::fsutil::private_dir(&home()?.join("bin"))
}

/// `$FERRY_HOME`, or the platform cache dir, created private to the user.
pub fn home() -> Result<PathBuf> {
    let home = std::env::var_os("FERRY_HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .or_else(crate::store::default_store_parent)
        .ok_or(Error::NoStoreDir)?;
    crate::fsutil::private_dir(&home)
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
    if let Some((ctx, meta)) = agent {
        return crate::agent::exec_cargo(&ctx, &meta, cargo, args, &cwd);
    }
    let mut command = Command::new(cargo);
    command.args(args).env(DEPTH_ENV, next_depth());
    if let (Some(src), Some(target)) = (
        std::env::var_os(WARM_SRC_ENV),
        std::env::var_os(WARM_TARGET_ENV),
    ) {
        let src = PathBuf::from(src);
        set_target(
            &mut command,
            workspace(cargo, args, &cwd, &[&src]).target(Path::new(&target)),
        );
    }
    Err(exec_error(cargo, command.exec()))
}

fn exec_error(cargo: &Path, source: std::io::Error) -> Error {
    Error::Sandbox(crate::sandbox::SandboxError::Exec {
        program: cargo.display().to_string(),
        source,
    })
}

/// Where in a target dir the target dirs of nested workspaces go.
pub(crate) const NESTED_TARGETS: &str = "ferry-ws";

/// Set for a base's warm commands: the checkout and target dir they build,
/// so that their cargo calls get the same per-workspace target dirs as an
/// agent's.
pub(crate) const WARM_SRC_ENV: &str = "FERRY_WARM_SRC";
pub(crate) const WARM_TARGET_ENV: &str = "FERRY_WARM_TARGET";

/// Points `command` at `target`, or for a workspace with no target here,
/// clears any inherited one so that it uses its own as plain cargo would.
pub(crate) fn set_target(command: &mut Command, target: Option<PathBuf>) {
    match target {
        Some(target) => command
            .env("CARGO_TARGET_DIR", &target)
            .env("CARGO_BUILD_BUILD_DIR", &target),
        None => command
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR"),
    };
}

/// The workspace a cargo invocation selects, relative to the checkout it
/// runs for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Workspace {
    /// Rooted at the checkout's root: the one the base warms by default.
    Root,
    /// Rooted at this path inside the checkout.
    Nested(PathBuf),
    /// Rooted outside the checkout, for example in another agent's worktree.
    Outside,
}

impl Workspace {
    /// Its target dir, given the checkout's.
    pub(crate) fn target(&self, target: &Path) -> Option<PathBuf> {
        match self {
            Self::Root => Some(target.to_path_buf()),
            // Cargo hashes path packages by their path relative to the
            // workspace root, so two workspaces sharing a target dir would
            // overwrite each other's units with the same names.
            Self::Nested(rel) => Some(target.join(NESTED_TARGETS).join(rel)),
            Self::Outside => None,
        }
    }
}

/// Finds the workspace `args` select when cargo runs in `cwd`. `checkout`
/// lists the paths the checkout is reachable at.
pub(crate) fn workspace(
    cargo: &Path,
    args: &[OsString],
    cwd: &Path,
    checkout: &[&Path],
) -> Workspace {
    let (globals, rest) = split_globals(args);
    let manifest = manifest_path(rest);
    if rest.is_empty() || (manifest.is_none() && checkout.iter().any(|c| same_dir(c, cwd))) {
        return Workspace::Root;
    }
    let mut locate = Command::new(cargo);
    locate
        .args(globals)
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if let Some(manifest) = manifest {
        locate.arg("--manifest-path").arg(manifest);
    }
    let located = locate
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| PathBuf::from(OsString::from_vec(out.stdout.trim_ascii_end().to_vec())));
    // Without a workspace the command itself fails, or (`cargo new`) does not
    // build anything.
    let Some(root) = located
        .as_deref()
        .and_then(Path::parent)
        .and_then(|r| std::fs::canonicalize(r).ok())
    else {
        return Workspace::Root;
    };
    for dir in checkout {
        if let Some(rel) = std::fs::canonicalize(dir)
            .ok()
            .and_then(|dir| root.strip_prefix(dir).ok().map(Path::to_path_buf))
        {
            return if rel.as_os_str().is_empty() {
                Workspace::Root
            } else {
                Workspace::Nested(rel)
            };
        }
    }
    Workspace::Outside
}

/// Splits off the options before cargo's subcommand (`+toolchain`, `-C dir`,
/// `-Z flag`, `--config k=v`, ...), which `locate-project` needs as well.
fn split_globals(args: &[OsString]) -> (&[OsString], &[OsString]) {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let arg = arg.as_bytes();
        if (i == 0 && arg.starts_with(b"+")) || arg.starts_with(b"-") {
            let takes_value =
                matches!(arg, b"-C" | b"-Z" | b"--config" | b"--color" | b"--explain");
            i += if takes_value { 2 } else { 1 };
        } else {
            break;
        }
    }
    args.split_at(i.min(args.len()))
}

fn manifest_path(args: &[OsString]) -> Option<&OsStr> {
    let mut args = args.iter().take_while(|a| *a != "--");
    while let Some(arg) = args.next() {
        if arg == "--manifest-path" {
            return args.next().map(OsString::as_os_str);
        }
        if let Some(path) = arg.as_bytes().strip_prefix(b"--manifest-path=") {
            return Some(OsStr::from_bytes(path));
        }
    }
    None
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

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn global_options_are_split_from_the_subcommand() {
        let args = os(&[
            "+nightly",
            "-Zunstable-options",
            "-C",
            "sub",
            "--locked",
            "build",
            "-p",
            "x",
        ]);
        let (globals, rest) = split_globals(&args);
        assert_eq!(globals, &args[..5]);
        assert_eq!(rest, &args[5..]);
        let args = os(&["--config", "build.jobs=1", "test"]);
        assert_eq!(split_globals(&args).1, os(&["test"]));
        assert_eq!(split_globals(&os(&["-V"])).1, &[] as &[OsString]);
        assert_eq!(split_globals(&os(&["-C"])).1, &[] as &[OsString]);
    }

    #[test]
    fn manifest_path_stops_at_the_program_arguments() {
        let path = |args: &[&str]| manifest_path(&os(args)).map(OsStr::to_owned);
        assert_eq!(
            path(&["build", "--manifest-path", "a/Cargo.toml"]),
            Some("a/Cargo.toml".into())
        );
        assert_eq!(
            path(&["build", "--manifest-path=b/Cargo.toml"]),
            Some("b/Cargo.toml".into())
        );
        assert_eq!(path(&["run", "--", "--manifest-path", "c"]), None);
        assert_eq!(path(&["build"]), None);
    }

    #[test]
    fn nested_workspaces_get_their_own_target_dir() {
        let target = Path::new("/t");
        assert_eq!(Workspace::Root.target(target), Some(target.to_path_buf()));
        assert_eq!(
            Workspace::Nested("fuzz".into()).target(target),
            Some("/t/ferry-ws/fuzz".into())
        );
        assert_eq!(Workspace::Outside.target(target), None);
    }
}
