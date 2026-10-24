//! `ferry base --watch`: builds a base whenever a revision moves.
//!
//! A foreground poller, one per store. Building a base enters the sandbox
//! for the rest of the process, so each build runs in a child `ferry base`;
//! the watcher itself only reads git and the store (and runs gc).

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;

use crate::base::{self, BuildReport, GcReport};
use crate::context::Context;
use crate::error::{Error, IoContext, Result};
use crate::git::CommitId;
use crate::lock::FileLock;

#[derive(Debug, Clone)]
pub struct WatchOptions {
    /// The revision to follow.
    pub rev: String,
    pub interval: Duration,
    /// Run `ferry gc` after each new base.
    pub gc: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// The revision has pointed at `commit` for two polls in a row.
    Building {
        commit: CommitId,
    },
    Built {
        commit: CommitId,
        report: Box<BuildReport>,
    },
    /// The previous base stays current; `commit` is not retried until the
    /// revision moves.
    Failed {
        commit: CommitId,
        error: String,
    },
    Collected {
        report: GcReport,
    },
    GcFailed {
        error: String,
    },
}

/// Polls `opts.rev` until the process is interrupted, building a base for
/// each commit it settles on and reporting every step to `on_event`.
pub fn watch(
    ctx: &Context,
    opts: &WatchOptions,
    on_event: &mut dyn FnMut(&Event),
) -> Result<std::convert::Infallible> {
    let lock_path = ctx.store.watch_lock();
    let Some(_lock) = FileLock::try_exclusive(&lock_path)? else {
        let pids = crate::sandbox::lock_holders(&lock_path);
        let by = if pids.is_empty() {
            String::new()
        } else {
            let pids: Vec<String> = pids.iter().map(ToString::to_string).collect();
            format!(" (pid {})", pids.join(", "))
        };
        return Err(Error::Refused(format!(
            "another `ferry base --watch` is already watching this repository{by}"
        )));
    };
    let git_dir = ctx.repo.git_dir()?;
    let exe = std::env::current_exe().at("locate", "the ferry binary")?;
    // The last commit built or given up on, so neither is built again until
    // the revision moves.
    let mut done = base::current(ctx)?.map(|b| b.commit);
    let mut seen: Option<CommitId> = None;
    loop {
        // Every step of a rebase or bisect would otherwise get a base.
        let commit = if busy(&git_dir) {
            None
        } else {
            ctx.repo.resolve_commit(&opts.rev).ok()
        };
        if let Some(commit) = commit {
            if seen.as_ref() == Some(&commit) && done.as_ref() != Some(&commit) {
                on_event(&Event::Building {
                    commit: commit.clone(),
                });
                match build(&exe, &ctx.repo.toplevel, &commit) {
                    Ok(report) => {
                        on_event(&Event::Built {
                            commit: commit.clone(),
                            report: Box::new(report),
                        });
                        if opts.gc {
                            on_event(&match base::gc(ctx, false) {
                                Ok(report) => Event::Collected { report },
                                Err(e) => Event::GcFailed {
                                    error: e.to_string(),
                                },
                            });
                        }
                    }
                    Err(error) => on_event(&Event::Failed {
                        commit: commit.clone(),
                        error,
                    }),
                }
                done = Some(commit.clone());
            }
            seen = Some(commit);
        }
        std::thread::sleep(opts.interval);
    }
}

/// True while a rebase, merge, cherry-pick, revert or bisect is in progress
/// in the worktree whose git dir is `git_dir`.
fn busy(git_dir: &Path) -> bool {
    [
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
    ]
    .iter()
    .any(|name| git_dir.join(name).exists())
}

/// Runs `ferry --json base --rev <commit>` in `toplevel`, its build output
/// going to the watcher's stderr.
fn build(
    exe: &Path,
    toplevel: &Path,
    commit: &CommitId,
) -> std::result::Result<BuildReport, String> {
    let output = Command::new(exe)
        .arg("-C")
        .arg(toplevel)
        .args(["--json", "base", "--rev", commit.as_str()])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", exe.display()))?;
    if !output.status.success() {
        return Err(format!("`ferry base` exited with {}", output.status));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("unreadable `ferry base` output: {e}"))
}
