//! The Rust toolchain that will build a worktree.
//!
//! Detection runs `rustc -vV` and `cargo -V` with the worktree as the working
//! directory so rustup honours a `rust-toolchain.toml` there.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ToolchainError {
    #[error("`{program}` could not be run in {}: {detail}", dir.display())]
    Unavailable {
        program: &'static str,
        dir: PathBuf,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Beta,
    Nightly,
    Dev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toolchain {
    /// e.g. `1.99.0` or `1.100.0-nightly`.
    pub release: String,
    pub channel: Channel,
    pub host: String,
    /// Full `rustc -vV` output.
    pub rustc_verbose: String,
    /// `cargo -V` output.
    pub cargo_version: String,
}

impl Toolchain {
    pub fn detect(dir: &Path) -> Result<Self, ToolchainError> {
        let rustc_verbose = capture("rustc", &["-vV"], dir)?;
        let cargo_version = capture("cargo", &["-V"], dir)?;
        Ok(Self::parse(&rustc_verbose, cargo_version.trim()))
    }

    pub fn parse(rustc_verbose: &str, cargo_version: &str) -> Self {
        let field = |name: &str| {
            rustc_verbose
                .lines()
                .find_map(|l| l.strip_prefix(name)?.strip_prefix(": "))
                .unwrap_or_default()
                .trim()
                .to_owned()
        };
        let release = field("release");
        let channel = if release.contains("-nightly") {
            Channel::Nightly
        } else if release.contains("-beta") {
            Channel::Beta
        } else if release.contains("-dev") {
            Channel::Dev
        } else {
            Channel::Stable
        };
        Self {
            release,
            channel,
            host: field("host"),
            rustc_verbose: rustc_verbose.trim().to_owned(),
            cargo_version: cargo_version.to_owned(),
        }
    }

    pub fn is_nightly(&self) -> bool {
        matches!(self.channel, Channel::Nightly | Channel::Dev)
    }
}

fn capture(program: &'static str, args: &[&str], dir: &Path) -> Result<String, ToolchainError> {
    let unavailable = |detail: String| ToolchainError::Unavailable {
        program,
        dir: dir.to_path_buf(),
        detail,
    };
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| unavailable(e.to_string()))?;
    if !output.status.success() {
        return Err(unavailable(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STABLE: &str = "rustc 1.99.0 (b940084d7 2026-09-28)\nbinary: rustc\n\
        commit-hash: b940084d7\ncommit-date: 2026-09-28\nhost: x86_64-unknown-linux-gnu\n\
        release: 1.99.0\nLLVM version: 21.1.0\n";

    #[test]
    fn parses_stable() {
        let t = Toolchain::parse(STABLE, "cargo 1.99.0 (5f94df478 2026-08-27)");
        assert_eq!(t.release, "1.99.0");
        assert_eq!(t.channel, Channel::Stable);
        assert_eq!(t.host, "x86_64-unknown-linux-gnu");
        assert!(!t.is_nightly());
    }

    #[test]
    fn parses_nightly() {
        let v = STABLE.replace("release: 1.99.0", "release: 1.101.0-nightly");
        let t = Toolchain::parse(&v, "cargo 1.101.0-nightly");
        assert_eq!(t.channel, Channel::Nightly);
        assert!(t.is_nightly());
    }
}
