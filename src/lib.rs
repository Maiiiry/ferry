//! Ferry gives every parallel AI coding agent its own git worktree with an
//! instantly forked, already-warm Cargo build.
//!
//! The binary (`ferry`) is the supported interface; this library exists so the
//! CLI and the integration tests share one implementation. Its API is not
//! stable.

#[cfg(not(unix))]
compile_error!(
    "Ferry supports Linux and macOS only. Windows is not supported yet: \
     see https://github.com/patwrall/ferry#platform-support"
);

#[cfg(unix)]
pub mod agent;
#[cfg(unix)]
pub mod base;
#[cfg(unix)]
pub mod cli;
#[cfg(unix)]
pub mod config;
#[cfg(unix)]
pub mod context;
#[cfg(unix)]
pub mod doctor;
#[cfg(unix)]
pub mod error;
#[cfg(unix)]
pub mod fork;
#[cfg(unix)]
pub mod freshness;
#[cfg(unix)]
pub mod fsutil;
#[cfg(unix)]
pub mod git;
#[cfg(unix)]
pub mod hook;
#[cfg(unix)]
pub mod lock;
#[cfg(unix)]
pub mod platform;
#[cfg(unix)]
pub mod prune;
#[cfg(unix)]
pub mod remap;
#[cfg(unix)]
pub mod sandbox;
#[cfg(unix)]
pub mod shim;
#[cfg(unix)]
pub mod store;
#[cfg(unix)]
pub mod toolchain;
#[cfg(unix)]
pub mod watch;

#[cfg(unix)]
pub use error::{Error, Result};
