use std::io;
use std::path::{Path, PathBuf};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(transparent)]
    Git(#[from] crate::git::GitError),

    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),

    #[error(transparent)]
    Fork(#[from] crate::fork::ForkError),

    #[error(transparent)]
    Sandbox(#[from] crate::sandbox::SandboxError),

    #[error(transparent)]
    Toolchain(#[from] crate::toolchain::ToolchainError),

    #[error("metadata file {} is unreadable: {source}", path.display())]
    Metadata {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("no warm base exists for this repository")]
    NoBase,

    #[error("base {id} is missing or incomplete")]
    BaseMissing { id: String },

    #[error("base build failed: `{command}` exited with {status}")]
    BaseBuildFailed { command: String, status: String },

    #[error("agent `{0}` already exists")]
    AgentExists(String),

    #[error("no agent named `{0}`")]
    AgentNotFound(String),

    #[error("{0} is not inside any ferry agent worktree")]
    NotInAgent(PathBuf),

    #[error("invalid agent name `{name}`: {reason}")]
    InvalidName { name: String, reason: &'static str },

    #[error("agent `{name}` is still in use by running processes (pids: {pids})")]
    AgentBusy { name: String, pids: String },

    /// Ferry detected a state where continuing could produce a wrong build or
    /// destroy work. The message explains what was found.
    #[error("refusing to continue: {0}")]
    Refused(String),

    #[error("could not determine a storage directory; set FERRY_HOME")]
    NoStoreDir,
}

impl Error {
    /// A short, actionable suggestion printed under the error by the CLI.
    pub fn hint(&self) -> Option<String> {
        match self {
            Error::NoBase => Some("run `ferry base` to build one".into()),
            Error::BaseBuildFailed { .. } => Some(
                "fix the build, or set the warm commands in ferry.toml ([base] targets or commands)".into(),
            ),
            Error::BaseMissing { .. } => {
                Some("run `ferry base` to rebuild it, then `ferry gc` to clean up".into())
            }
            Error::AgentExists(name) => Some(format!(
                "pick another name or remove it with `ferry rm {name}`"
            )),
            Error::AgentNotFound(_) | Error::NotInAgent(_) => {
                Some("`ferry list` shows existing agents".into())
            }
            Error::AgentBusy { name, .. } => Some(format!(
                "stop those processes first, or pass --force to `ferry rm {name}`"
            )),
            Error::NoStoreDir => Some("export FERRY_HOME=/path/to/storage".into()),
            Error::Sandbox(e) => e.hint(),
            Error::Fork(e) => e.hint(),
            Error::Git(e) => e.hint(),
            _ => None,
        }
    }
}

/// Attaches the operation and path to an `io::Error`, because a bare
/// "Permission denied" is useless to someone debugging a build store.
pub trait IoContext<T> {
    fn at(self, op: &'static str, path: impl AsRef<Path>) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn at(self, op: &'static str, path: impl AsRef<Path>) -> Result<T> {
        self.map_err(|source| Error::Io {
            op,
            path: path.as_ref().to_path_buf(),
            source,
        })
    }
}
