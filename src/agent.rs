//! Spawning, running, listing and removing agents.
//!
//! INVARIANT: each agent owns its build state. An agent's commands only ever
//! see its own worktree and its own fork of the base; nothing an agent does
//! writes to the base or to another agent.
//!
//! The agent a command belongs to is always derived from its working
//! directory, never from inherited environment variables: agent tools copy
//! their session environment into sub-agents that work in other worktrees,
//! so an inherited `CARGO_TARGET_DIR` (or agent name) would point a sub-agent
//! at someone else's build state — the shared-target bug Ferry prevents.

use std::convert::Infallible;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::base::{self, BaseMeta};
use crate::config::SandboxMode;
use crate::context::Context;
use crate::error::{Error, IoContext, Result};
use crate::fork::{self, BackendKind, ForkPaths, ForkStats};
use crate::freshness::{self, FreshnessMode, StampReport};
use crate::fsutil;
use crate::git::{self, CommitId};
use crate::lock::FileLock;
use crate::platform::Capabilities;
use crate::sandbox::{self, Isolation, Plan};
use crate::shim;
use crate::store::{AgentName, AgentPaths, BaseId};

/// Set inside every sandbox to the agent's name, for scripts and prompts.
/// Ferry itself never reads it.
pub const SANDBOX_ENV: &str = "FERRY_SANDBOX";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    /// `ferry spawn` has not finished (or crashed). `ferry rm` cleans it up.
    Creating,
    Ready,
}

/// Who created the agent. Agents created by an agent tool's hook may be
/// reaped by `ferry gc` once they are clean and idle, because some tools
/// never call the matching removal hook.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    #[default]
    Cli,
    ClaudeHook,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMeta {
    pub name: AgentName,
    pub state: AgentState,
    pub base: BaseId,
    /// Commit the agent's branch started at.
    pub from: CommitId,
    pub branch: String,
    pub worktree: PathBuf,
    pub backend: BackendKind,
    pub isolation: Isolation,
    pub freshness: FreshnessMode,
    pub created_unix: u64,
    pub stamp: StampReport,
    #[serde(default)]
    pub origin: Origin,
}

#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    pub from: Option<String>,
    pub path: Option<PathBuf>,
    pub branch: Option<String>,
    pub origin: Origin,
}

#[derive(Debug, Serialize)]
pub struct SpawnReport {
    pub agent: AgentMeta,
    pub backend_reason: String,
    pub fork: ForkStats,
    pub elapsed_secs: f64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentStatus {
    #[serde(flatten)]
    pub meta: AgentMeta,
    pub base_is_current: bool,
    pub worktree_exists: bool,
    /// Bytes of build state private to this agent (see
    /// [`fork::Backend::private_bytes`]).
    pub private_bytes: Option<u64>,
    /// PIDs currently running inside the agent's sandbox.
    pub running: Vec<u32>,
}

#[derive(Debug, Serialize)]
pub struct RemoveReport {
    pub name: AgentName,
    pub worktree: PathBuf,
    pub branch: String,
    pub branch_deleted: bool,
    /// Commits on the branch beyond where the agent started, if it was kept.
    pub branch_commits: u64,
}

/// What an agent tool needs to know to use an agent.
#[derive(Debug, Serialize)]
pub struct EnvReport {
    pub name: AgentName,
    pub worktree: PathBuf,
    pub branch: String,
    pub base: BaseId,
    pub backend: BackendKind,
    pub isolation: Isolation,
    /// Target dir as seen by cargo when it runs through Ferry.
    pub build_target_dir: PathBuf,
    /// Host path of the agent's target dir, when one exists outside the
    /// sandbox (reflink and copy backends).
    pub host_target_dir: Option<PathBuf>,
    /// Directory holding Ferry's `cargo` shim; put it first on `PATH`.
    pub shim_dir: PathBuf,
    /// Prefix that runs a command in the agent's build context.
    pub run_prefix: Vec<String>,
}

pub fn spawn(ctx: &Context, name: &AgentName, opts: SpawnOptions) -> Result<SpawnReport> {
    let started = Instant::now();
    let mut warnings = Vec::new();
    let from = ctx
        .repo
        .resolve_commit(opts.from.as_deref().unwrap_or("HEAD"))?;
    let paths = ctx.store.agent(name);
    let caps = Capabilities::detect(ctx.store.root());
    let branch = opts
        .branch
        .unwrap_or_else(|| format!("{}{name}", ctx.settings.branch_prefix));
    let worktree = match opts.path {
        Some(p) if p.is_absolute() => p,
        Some(p) => std::env::current_dir()
            .at("read", "current directory")?
            .join(p),
        None => ctx
            .settings
            .worktree_parent
            .as_ref()
            .ok_or(Error::NoStoreDir)?
            .join(ctx.store.key())
            .join(name.as_str()),
    };

    let (base, mut meta, _agent_lock) = {
        let _store = ctx.store.lock()?;
        if paths.dir.exists() {
            return Err(Error::AgentExists(name.to_string()));
        }
        if ctx.repo.branch_tip(&branch)?.is_some() {
            return Err(Error::Refused(format!(
                "branch `{branch}` already exists; pass --branch to use another name"
            )));
        }
        if worktree.exists()
            && fs::read_dir(&worktree)
                .at("read directory", &worktree)?
                .next()
                .is_some()
        {
            return Err(Error::Refused(format!(
                "{} already exists and is not empty",
                worktree.display()
            )));
        }
        let base = choose_base(ctx, &from)?;
        let isolation = agent_isolation(ctx, &caps, &base, &mut warnings)?;
        fs::create_dir_all(&paths.dir).at("create directory", &paths.dir)?;
        let lock = FileLock::exclusive(&paths.lock, &format!("agent {name}"))?;
        let meta = AgentMeta {
            name: name.clone(),
            state: AgentState::Creating,
            base: base.id.clone(),
            from: from.clone(),
            branch: branch.clone(),
            worktree: worktree.clone(),
            // Provisional until the backend is chosen below.
            backend: BackendKind::Copy,
            isolation,
            freshness: base.freshness,
            created_unix: fsutil::now_unix(),
            stamp: StampReport::default(),
            origin: opts.origin,
        };
        fsutil::write_json_atomic(&paths.meta, &meta)?;
        (base, meta, lock)
    };

    match populate(ctx, &caps, &paths, &base, &mut meta, &mut warnings) {
        Ok((backend_reason, fork_stats)) => Ok(SpawnReport {
            agent: meta,
            backend_reason,
            fork: fork_stats,
            elapsed_secs: started.elapsed().as_secs_f64(),
            warnings,
        }),
        Err(e) => {
            let _ = rollback(ctx, &paths, &meta);
            Err(e)
        }
    }
}

fn populate(
    ctx: &Context,
    caps: &Capabilities,
    paths: &AgentPaths,
    base: &BaseMeta,
    meta: &mut AgentMeta,
    warnings: &mut Vec<String>,
) -> Result<(String, ForkStats)> {
    let base_paths = ctx.store.base(&base.id);
    if !base_paths.src.is_dir() || !base_paths.target.is_dir() {
        return Err(Error::Refused(format!(
            "base {} is missing its checkout or target dir, so stamping cannot be verified",
            base.id.short()
        )));
    }
    ctx.repo
        .add_branch_worktree(&meta.worktree, &meta.branch, &meta.from)?;

    let base_tree = ctx.repo.ls_tree(&base.commit)?;
    let index = git::ls_files(&meta.worktree)?;
    let plan = freshness::plan(&base_tree, &base_paths.src, &index, &meta.worktree)?;
    meta.stamp = freshness::apply(&plan)?;
    git::refresh_index(&meta.worktree)?;
    if meta.stamp.skipped > 0 {
        warnings.push(format!(
            "{} of {} tracked files differ from base {}; crates using them rebuild on the first build",
            meta.stamp.skipped,
            meta.stamp.tracked,
            base.id.short()
        ));
    }

    let choice = fork::select(ctx.settings.backend, caps, meta.isolation)?;
    meta.backend = choice.kind;
    fsutil::write_json_atomic(&paths.meta, &*meta)?;
    let stats = fork::backend(choice.kind)?.fork(&fork_paths(paths, &base_paths.target))?;
    if meta.isolation == Isolation::None {
        // The agent builds at different absolute paths than the base did.
        let old_roots = match base.isolation {
            Isolation::Namespace => vec![ctx.store.canonical_src(), ctx.store.canonical_target()],
            Isolation::None => vec![base_paths.src.clone(), base_paths.target.clone()],
        };
        let invalidated = freshness::invalidate_relocated_units(&paths.target, &old_roots)?;
        if !invalidated.is_empty() {
            warnings.push(format!(
                "{} unit(s) captured the base's absolute paths (e.g. env!(\"CARGO_MANIFEST_DIR\")) and will rebuild",
                invalidated.len()
            ));
        }
        if base.remap_paths {
            crate::remap::prepare(&meta.worktree, &paths.target)?;
        }
    }

    meta.state = AgentState::Ready;
    fsutil::write_json_atomic(&paths.meta, &*meta)?;
    Ok((choice.reason, stats))
}

fn rollback(ctx: &Context, paths: &AgentPaths, meta: &AgentMeta) -> Result<()> {
    if meta.worktree.exists() {
        ctx.repo.remove_worktree(&meta.worktree, true)?;
    }
    if ctx.repo.branch_tip(&meta.branch)?.as_ref() == Some(&meta.from) {
        ctx.repo.delete_branch(&meta.branch)?;
    }
    fsutil::remove_dir_all_force(&paths.dir)
}

/// The base an agent starting at `from` should fork: a ready base built
/// from exactly `from` if one exists (the current one preferred), otherwise
/// the current base, with stamping limited to files identical to it.
fn choose_base(ctx: &Context, from: &CommitId) -> Result<BaseMeta> {
    let current = base::current(ctx)?;
    if let Some(cur) = &current
        && &cur.commit == from
    {
        return Ok(cur.clone());
    }
    if let Some(exact) = base::ready_bases(ctx)?
        .into_iter()
        .find(|b| &b.commit == from)
    {
        return Ok(exact);
    }
    current.ok_or(Error::NoBase)
}

fn agent_isolation(
    ctx: &Context,
    caps: &Capabilities,
    base: &BaseMeta,
    warnings: &mut Vec<String>,
) -> Result<Isolation> {
    if base.isolation == Isolation::None || ctx.settings.sandbox == SandboxMode::Off {
        return Ok(Isolation::None);
    }
    if caps.userns.is_ok() {
        return Ok(Isolation::Namespace);
    }
    if ctx.settings.sandbox == SandboxMode::Namespace {
        return Err(Error::Sandbox(sandbox::SandboxError::Unsupported(
            "user namespaces are unavailable".into(),
        )));
    }
    warnings.push(
        "the base was built in a namespace sandbox, but namespaces are unavailable now; \
         this agent builds at its host path without incremental reuse"
            .into(),
    );
    Ok(Isolation::None)
}

pub fn fork_paths(paths: &AgentPaths, base_target: &Path) -> ForkPaths {
    ForkPaths {
        base_target: base_target.to_path_buf(),
        agent_target: paths.target.clone(),
        upper: paths.upper.clone(),
        work: paths.work.clone(),
    }
}

pub fn load(ctx: &Context, name: &AgentName) -> Result<AgentMeta> {
    let paths = ctx.store.agent(name);
    if !paths.meta.exists() {
        return Err(Error::AgentNotFound(name.to_string()));
    }
    fsutil::read_json(&paths.meta)
}

pub fn all_meta(ctx: &Context) -> Result<Vec<AgentMeta>> {
    let mut out = Vec::new();
    for name in ctx.store.agent_names()? {
        let paths = ctx.store.agent(&name);
        if paths.meta.exists() {
            out.push(fsutil::read_json(&paths.meta)?);
        }
    }
    Ok(out)
}

/// The agent whose worktree contains `dir`, or `None`.
///
/// Inside a namespace sandbox `dir` may be under the canonical source path;
/// it then belongs to whichever agent's worktree is mounted there, which is
/// checked by inode rather than trusted from the environment.
pub fn find_by_dir(ctx: &Context, dir: &Path) -> Result<Option<AgentMeta>> {
    let dir = fs::canonicalize(dir).at("resolve", dir)?;
    let canonical = ctx.store.canonical_src();
    let agents = all_meta(ctx)?;
    if dir.starts_with(&canonical) {
        return Ok(agents
            .into_iter()
            .find(|a| same_file(&canonical, &a.worktree)));
    }
    Ok(agents
        .into_iter()
        .find(|a| fs::canonicalize(&a.worktree).is_ok_and(|root| dir.starts_with(root))))
}

pub fn find_by_cwd(ctx: &Context, cwd: &Path) -> Result<AgentMeta> {
    find_by_dir(ctx, cwd)?.ok_or_else(|| Error::NotInAgent(cwd.to_path_buf()))
}

pub fn list(ctx: &Context) -> Result<Vec<AgentStatus>> {
    let current = ctx.store.current_base()?;
    let mut out = Vec::new();
    for meta in all_meta(ctx)? {
        let paths = ctx.store.agent(&meta.name);
        let base_target = ctx.store.base(&meta.base).target;
        let private_bytes = fork::backend(meta.backend)
            .ok()
            .and_then(|b| b.private_bytes(&fork_paths(&paths, &base_target)).ok());
        out.push(AgentStatus {
            base_is_current: current.as_ref() == Some(&meta.base),
            worktree_exists: meta.worktree.is_dir(),
            private_bytes,
            running: sandbox::users(&paths.ns_record).unwrap_or_default(),
            meta,
        });
    }
    Ok(out)
}

pub fn remove(ctx: &Context, name: &AgentName, force: bool) -> Result<RemoveReport> {
    let _store = ctx.store.lock()?;
    remove_locked(ctx, name, force)
}

/// [`remove`] for callers that already hold the store lock.
fn remove_locked(ctx: &Context, name: &AgentName, force: bool) -> Result<RemoveReport> {
    let meta = load(ctx, name)?;
    let paths = ctx.store.agent(name);
    let _agent = FileLock::exclusive(&paths.lock, &format!("agent {name}"))?;

    let running = sandbox::users(&paths.ns_record)?;
    if !running.is_empty() && !force {
        return Err(Error::AgentBusy {
            name: name.to_string(),
            pids: running
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        });
    }
    if meta.worktree.is_dir() {
        if !force && git::is_dirty(&meta.worktree)? {
            return Err(Error::Refused(format!(
                "{} has uncommitted changes; commit them or pass --force to discard them",
                meta.worktree.display()
            )));
        }
        ctx.repo.remove_worktree(&meta.worktree, force)?;
    } else {
        ctx.repo.prune_worktrees()?;
    }

    let tip = ctx.repo.branch_tip(&meta.branch)?;
    let (branch_deleted, branch_commits) = match &tip {
        None => (false, 0),
        Some(tip) if tip == &meta.from => {
            ctx.repo.delete_branch(&meta.branch)?;
            (true, 0)
        }
        Some(tip) => (false, git::commits_ahead(&ctx.repo, &meta.from, tip)?),
    };

    let base_target = ctx.store.base(&meta.base).target;
    fork::remove(&fork_paths(&paths, &base_target))?;
    fsutil::remove_dir_all_force(&paths.dir)?;
    Ok(RemoveReport {
        name: name.clone(),
        worktree: meta.worktree,
        branch: meta.branch,
        branch_deleted,
        branch_commits,
    })
}

/// Removes hook-created agents that their tool abandoned: clean worktree, no
/// commits beyond the starting point, nothing running, and not locked by
/// the tool (`git worktree lock`). Never touches agents created with
/// `ferry spawn`. Caller must hold the store lock.
pub fn reap_abandoned(ctx: &Context, dry_run: bool) -> Result<Vec<AgentName>> {
    let locked = git::locked_worktrees(&ctx.repo)?;
    let mut reaped = Vec::new();
    for meta in all_meta(ctx)? {
        if meta.origin != Origin::ClaudeHook || meta.state != AgentState::Ready {
            continue;
        }
        let worktree = fs::canonicalize(&meta.worktree).unwrap_or_else(|_| meta.worktree.clone());
        if locked.iter().any(|l| l == &worktree) {
            continue;
        }
        let paths = ctx.store.agent(&meta.name);
        let Some(_agent) = FileLock::try_exclusive(&paths.lock)? else {
            continue;
        };
        if !sandbox::users(&paths.ns_record)?.is_empty()
            || (meta.worktree.is_dir() && git::is_dirty(&meta.worktree)?)
            || ctx
                .repo
                .branch_tip(&meta.branch)?
                .is_some_and(|tip| tip != meta.from)
        {
            continue;
        }
        drop(_agent);
        if !dry_run {
            remove_locked(ctx, &meta.name, false)?;
        }
        reaped.push(meta.name);
    }
    Ok(reaped)
}

/// The build context for `meta`.
pub fn plan(ctx: &Context, meta: &AgentMeta) -> Result<Plan> {
    let paths = ctx.store.agent(&meta.name);
    let base_target = ctx.store.base(&meta.base).target;
    let fork_paths = fork_paths(&paths, &base_target);
    let target = fork::backend(meta.backend)?.target_spec(&fork_paths);
    let remap = meta.isolation == Isolation::None && base::load(ctx, &meta.base)?.remap_paths;
    Ok(Plan {
        isolation: meta.isolation,
        src: meta.worktree.clone(),
        target,
        canonical_src: ctx.store.canonical_src(),
        canonical_target: ctx.store.canonical_target(),
        ns_record: (meta.backend == BackendKind::Overlay).then(|| paths.ns_record.clone()),
        remap_wrapper: if remap {
            Some(crate::remap::ensure_wrapper(ctx)?)
        } else {
            None
        },
    })
}

/// Environment for cargo in `plan`. Both the target dir and Cargo's build
/// dir point at the fork so a user's `build.build-dir` or an inherited
/// `CARGO_TARGET_DIR` cannot redirect artifacts into a shared location.
pub fn cargo_env(plan: &Plan, freshness: FreshnessMode) -> Vec<(OsString, OsString)> {
    let target = plan.build_target().as_os_str().to_owned();
    let mut env: Vec<(OsString, OsString)> = vec![
        ("CARGO_TARGET_DIR".into(), target.clone()),
        ("CARGO_BUILD_BUILD_DIR".into(), target),
    ];
    env.extend(
        freshness::cargo_env(freshness)
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v))),
    );
    if let Some(wrapper) = &plan.remap_wrapper {
        env.extend(crate::remap::cargo_env(wrapper));
    }
    env
}

/// `ferry run` / `ferry shell`: replaces the current process with `argv`
/// inside the agent's sandbox. The working directory stays a host path (so
/// tools that key state on their cwd, like agent transcripts, keep one
/// entry per agent); cargo is routed through the shim, which moves each
/// cargo invocation to the canonical paths.
pub fn exec_session(
    ctx: &Context,
    meta: &AgentMeta,
    argv: &[OsString],
    host_cwd: &Path,
) -> Result<Infallible> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| Error::Refused("no command given".into()))?;
    let plan = plan(ctx, meta)?;
    let shim_dir = shim::ensure(ctx)?;
    enter(ctx, meta, &plan)?;
    let cwd = if inside(host_cwd, &meta.worktree) {
        host_cwd.to_path_buf()
    } else {
        meta.worktree.clone()
    };
    let err = std::process::Command::new(program)
        .args(args)
        .current_dir(&cwd)
        .env("PATH", shim::path_with(&shim_dir))
        .env("PWD", &cwd)
        .env(SANDBOX_ENV, meta.name.as_str())
        .env("FERRY_WORKTREE", &meta.worktree)
        .exec();
    Err(exec_error(program, err))
}

/// The `cargo` shim: replaces the current process with the real cargo,
/// running in the agent's sandbox at the canonical source path with the
/// agent's own target dir.
pub fn exec_cargo(
    ctx: &Context,
    meta: &AgentMeta,
    cargo: &Path,
    args: &[OsString],
    host_cwd: &Path,
) -> Result<Infallible> {
    let plan = plan(ctx, meta)?;
    enter(ctx, meta, &plan)?;
    let cwd = plan.map_cwd(host_cwd);
    let err = std::process::Command::new(cargo)
        .args(args)
        .current_dir(&cwd)
        .env("PWD", &cwd)
        .envs(cargo_env(&plan, meta.freshness))
        .exec();
    Err(exec_error(cargo.as_os_str(), err))
}

/// Enters `meta`'s sandbox unless this process is already inside it.
fn enter(ctx: &Context, meta: &AgentMeta, plan: &Plan) -> Result<()> {
    if plan.isolation == Isolation::Namespace && same_file(&plan.canonical_src, &plan.src) {
        return Ok(());
    }
    if meta.state != AgentState::Ready {
        return Err(Error::Refused(format!(
            "agent `{}` was not fully created; remove it with `ferry rm {}`",
            meta.name, meta.name
        )));
    }
    if base::load_ready(&ctx.store.base(&meta.base))?.is_none() {
        return Err(Error::BaseMissing {
            id: meta.base.to_string(),
        });
    }
    let paths = ctx.store.agent(&meta.name);
    let _lock = FileLock::exclusive(&paths.lock, &format!("agent {}", meta.name))?;
    sandbox::enter(plan)?;
    Ok(())
}

pub fn env_report(ctx: &Context, meta: &AgentMeta) -> Result<EnvReport> {
    let plan = plan(ctx, meta)?;
    let host_target_dir = match &plan.target {
        fork::TargetSpec::Dir(dir) => Some(dir.clone()),
        fork::TargetSpec::Overlay { .. } => None,
    };
    Ok(EnvReport {
        name: meta.name.clone(),
        worktree: meta.worktree.clone(),
        branch: meta.branch.clone(),
        base: meta.base.clone(),
        backend: meta.backend,
        isolation: meta.isolation,
        build_target_dir: plan.build_target(),
        host_target_dir,
        shim_dir: shim::ensure(ctx)?,
        run_prefix: vec![
            "ferry".into(),
            "run".into(),
            meta.name.to_string(),
            "--".into(),
        ],
    })
}

fn inside(dir: &Path, root: &Path) -> bool {
    match (fs::canonicalize(dir), fs::canonicalize(root)) {
        (Ok(dir), Ok(root)) => dir.starts_with(root),
        _ => false,
    }
}

fn exec_error(program: &std::ffi::OsStr, source: std::io::Error) -> Error {
    Error::Sandbox(sandbox::SandboxError::Exec {
        program: program.to_string_lossy().into_owned(),
        source,
    })
}

/// True if `a` and `b` are the same directory (same device and inode).
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}
