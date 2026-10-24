//! Experimental: path-independent rustc incremental state for builds that run
//! without the namespace sandbox (macOS, or Linux without user namespaces).
//!
//! rustc discards or invalidates incremental state when the absolute working
//! directory or `--out-dir` differ from the previous session. Ferry's
//! `ferry-rustc` `RUSTC_WORKSPACE_WRAPPER` (a symlink to the ferry binary)
//! adds `-Zremap-cwd-prefix=.` and rewrites absolute worktree and target paths
//! in rustc's argv to relative ones, for workspace members only. The target
//! dir is reached through a link inside the source root, so the relative path
//! from source to target is identical in the base and every agent. See
//! research/macos-and-path-independent-incremental.md.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::context::Context;
use crate::error::Result;
use crate::toolchain::Toolchain;

/// Name of the wrapper symlink in the shim directory.
pub const WRAPPER_NAME: &str = "ferry-rustc";

/// Name of the link to the target dir inside a source root.
pub const TARGET_LINK: &str = ".ferry-target";

/// Whether `toolchain` supports remapping; the error says why not.
pub fn supported(toolchain: &Toolchain) -> std::result::Result<(), String> {
    let _ = toolchain;
    todo!("remap::supported")
}

/// `<store parent>/bin/ferry-rustc`, (re)pointed at the running binary. The
/// path must be stable: Cargo hashes the wrapper path into workspace members'
/// artifact names.
pub fn ensure_wrapper(ctx: &Context) -> Result<PathBuf> {
    let _ = ctx;
    todo!("remap::ensure_wrapper")
}

/// Creates `<src_root>/.ferry-target -> target` (hidden from `git status`)
/// and returns the link path, which builds use as their target dir.
pub fn prepare(src_root: &Path, target: &Path) -> Result<PathBuf> {
    let _ = (src_root, target);
    todo!("remap::prepare")
}

/// Extra cargo environment in remap mode.
pub fn cargo_env(wrapper: &Path) -> Vec<(OsString, OsString)> {
    vec![(
        "RUSTC_WORKSPACE_WRAPPER".into(),
        wrapper.as_os_str().to_owned(),
    )]
}

/// True if the process was started through the wrapper symlink.
pub fn invoked_as_wrapper() -> bool {
    std::env::args_os()
        .next()
        .and_then(|a| Path::new(&a).file_name().map(|n| n == WRAPPER_NAME))
        .unwrap_or(false)
}

/// Entry point when invoked as `ferry-rustc <rustc> <args...>`.
pub fn main() -> ExitCode {
    todo!("remap::main")
}
