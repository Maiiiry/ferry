//! Agent-tool hook handlers (`ferry hook <event>`).
//!
//! Claude Code: <https://code.claude.com/docs/en/hooks>. Its `WorktreeCreate`
//! hook replaces git worktree creation for `claude --worktree`, sub-agents
//! with `isolation: worktree` and background sessions; the hook must print
//! the new worktree's absolute path as the last line of stdout, and anything
//! else on stdout would be taken as that path. `WorktreeRemove` receives
//! `worktree_path`; exiting non-zero leaves the directory in place.

use std::io::{Read, Write};
use std::path::PathBuf;

use serde::Deserialize;

use crate::agent::{self, Origin, SpawnOptions};
use crate::context::Context;
use crate::error::{Error, IoContext, Result};
use crate::store::AgentName;

#[derive(Debug, Deserialize)]
struct WorktreeCreateInput {
    cwd: PathBuf,
    name: String,
}

#[derive(Debug, Deserialize)]
struct WorktreeRemoveInput {
    cwd: Option<PathBuf>,
    worktree_path: PathBuf,
}

/// Handles Claude Code's `WorktreeCreate`: spawns (or resumes) an agent and
/// prints its worktree path. Returns the path.
pub fn claude_worktree_create(stdin: &mut dyn Read, stdout: &mut dyn Write) -> Result<PathBuf> {
    let input: WorktreeCreateInput = read_input(stdin)?;
    let ctx = Context::discover(&input.cwd)?;
    let name = agent_name(&input.name)?;
    let worktree = match agent::load(&ctx, &name) {
        Ok(meta) if meta.state == agent::AgentState::Ready => {
            eprintln!("ferry: reusing agent {name}");
            meta.worktree
        }
        Ok(_) => {
            return Err(Error::Refused(format!(
                "agent `{name}` exists but was not fully created; run `ferry rm {name}`"
            )));
        }
        Err(Error::AgentNotFound(_)) => {
            let report = agent::spawn(
                &ctx,
                &name,
                SpawnOptions {
                    origin: Origin::ClaudeHook,
                    ..SpawnOptions::default()
                },
            )?;
            for w in &report.warnings {
                eprintln!("ferry: warning: {w}");
            }
            eprintln!(
                "ferry: spawned {name} ({} fork, {:.2}s)",
                report.agent.backend, report.elapsed_secs
            );
            report.agent.worktree
        }
        Err(e) => return Err(e),
    };
    writeln!(stdout, "{}", worktree.display()).at("write", "stdout")?;
    Ok(worktree)
}

/// Handles Claude Code's `WorktreeRemove`. Refuses (non-zero exit, worktree
/// left in place) when the worktree has uncommitted changes or processes
/// still run in it; commits stay on the agent's branch.
pub fn claude_worktree_remove(stdin: &mut dyn Read) -> Result<()> {
    let input: WorktreeRemoveInput = read_input(stdin)?;
    let start = input.cwd.unwrap_or_else(|| input.worktree_path.clone());
    let ctx = Context::discover(&start)?;
    if !input.worktree_path.exists() {
        return Ok(());
    }
    let meta = agent::find_by_dir(&ctx, &input.worktree_path)?
        .ok_or_else(|| Error::NotInAgent(input.worktree_path.clone()))?;
    let report = agent::remove(&ctx, &meta.name, false)?;
    if !report.branch_deleted && report.branch_commits > 0 {
        eprintln!(
            "ferry: kept branch {} ({} new commit(s))",
            report.branch, report.branch_commits
        );
    }
    Ok(())
}

/// Handles Claude Code's `SessionStart`: puts Ferry's cargo shim first on
/// `PATH` for every Bash command via `$CLAUDE_ENV_FILE`. Only `PATH` is
/// written: the shim resolves the agent per call from the working
/// directory, so it stays correct in sub-agents that inherit this
/// environment.
pub fn claude_session_start(stdin: &mut dyn Read) -> Result<()> {
    #[derive(Deserialize)]
    struct Input {
        cwd: PathBuf,
    }
    let input: Input = read_input(stdin)?;
    let Some(env_file) = std::env::var_os("CLAUDE_ENV_FILE") else {
        return Ok(());
    };
    let ctx = match Context::discover(&input.cwd) {
        Ok(ctx) => ctx,
        Err(Error::Git(_)) => return Ok(()),
        Err(e) => return Err(e),
    };
    let shim_dir = crate::shim::ensure(&ctx)?;
    let line = format!(
        "export PATH={}:\"$PATH\"\n",
        crate::config::shell_quote(&shim_dir.to_string_lossy())
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&env_file)
        .at("open", &env_file)?;
    file.write_all(line.as_bytes()).at("write", &env_file)
}

/// Claude names worktrees like `bright-running-fox` or, for sub-agents,
/// `agent-<hex>`; anything outside Ferry's name alphabet becomes `-`.
fn agent_name(raw: &str) -> Result<AgentName> {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    trimmed.parse()
}

fn read_input<T: for<'de> Deserialize<'de>>(stdin: &mut dyn Read) -> Result<T> {
    let mut buf = Vec::new();
    stdin.read_to_end(&mut buf).at("read", "stdin")?;
    serde_json::from_slice(&buf).map_err(|source| Error::Metadata {
        path: PathBuf::from("<hook input on stdin>"),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_names_map_to_agent_names() {
        assert_eq!(agent_name("t1").unwrap().as_str(), "t1");
        assert_eq!(
            agent_name("agent-abd7add2abe2a458a").unwrap().as_str(),
            "agent-abd7add2abe2a458a"
        );
        assert_eq!(
            agent_name("feat/login page").unwrap().as_str(),
            "feat-login-page"
        );
        assert!(agent_name("///").is_err());
    }

    #[test]
    fn remove_input_parses_measured_shape() {
        let json = br#"{"session_id":"s","transcript_path":"/t","cwd":"/repo","hook_event_name":"WorktreeRemove","worktree_path":"/w"}"#;
        let input: WorktreeRemoveInput = read_input(&mut &json[..]).unwrap();
        assert_eq!(input.worktree_path, PathBuf::from("/w"));
    }
}
