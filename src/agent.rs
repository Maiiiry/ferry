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
pub const SANDBOX_ENV: &str = "FERRY_AGENT";

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
    /// Whether any process started for the agent is still alive.
    pub busy: bool,
    /// The PIDs of those processes that could be attributed.
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
    copy_untracked_lockfile(&base_paths.src, &meta.worktree)?;
    if base.remap_paths && meta.isolation == Isolation::None {
        // Before stamping, so the root directory's entries match the base's.
        crate::remap::prepare(&meta.worktree, &paths.target)?;
    }

    let base_tree = ctx.repo.ls_tree(&base.commit)?;
    let index = git::ls_files(&meta.worktree)?;
    let plan = freshness::plan(&base_tree, &base_paths.src, &index, &meta.worktree)?;
    let differing = plan
        .skipped()
        .iter()
        .filter(|(_, reason)| *reason != freshness::SkipReason::NotRegularFile)
        .count();
    meta.stamp = freshness::apply(&plan)?;
    git::refresh_index(&meta.worktree)?;
    if differing > 0 {
        warnings.push(format!(
            "{differing} of {} tracked files differ from base {}; crates using them rebuild on the first build",
            meta.stamp.tracked,
            base.id.short()
        ));
    }

    let choice = fork::select(ctx.settings.backend, caps, meta.isolation)?;
    meta.backend = choice.kind;
    fsutil::write_json_atomic(&paths.meta, &*meta)?;
    let stats = fork::backend(choice.kind)?.fork(&fork_paths(paths, &base_paths.target))?;
    let _ = crate::platform::exclude_from_backups(&paths.dir);
    if meta.isolation == Isolation::None {
        // The agent builds at a different absolute source path than the
        // base did. Only the source root matters: the base's target dir is
        // immutable and outlives the agent, so a captured OUT_DIR still reads
        // what this agent's own build script would produce, and Cargo
        // re-runs dependents whenever that output changes.
        let old_src = match base.isolation {
            Isolation::Namespace => ctx.store.canonical_src(),
            Isolation::None => base_paths.src.clone(),
        };
        let invalidated = freshness::invalidate_relocated_units(&paths.target, &[old_src])?;
        if !invalidated.is_empty() {
            warnings.push(format!(
                "{} unit(s) captured the base's absolute paths (e.g. env!(\"CARGO_MANIFEST_DIR\")) and will rebuild",
                invalidated.len()
            ));
        }
    }

    meta.state = AgentState::Ready;
    fsutil::write_json_atomic(&paths.meta, &*meta)?;
    Ok((choice.reason, stats))
}

/// Gives the agent the base's `Cargo.lock` when the repository does not
/// track one, so the agent resolves the same dependency versions the base
/// was built with instead of re-resolving (and rebuilding) on first build.
pub(crate) fn copy_untracked_lockfile(base_src: &Path, worktree: &Path) -> Result<()> {
    let (from, to) = (base_src.join("Cargo.lock"), worktree.join("Cargo.lock"));
    if !from.is_file() || to.exists() {
        return Ok(());
    }
    fs::copy(&from, &to).at("copy", &from)?;
    let mtime = fs::metadata(&from)
        .and_then(|m| m.modified())
        .at("stat", &from)?;
    fs::File::options()
        .write(true)
        .open(&to)
        .and_then(|f| f.set_modified(mtime))
        .at("set times on", &to)
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
    if caps.userns.is_inconclusive() {
        return Err(Error::Refused(
            "could not tell whether the namespace sandbox works (the probe did not answer); \
             retry, or set FERRY_SANDBOX=off to spawn without the sandbox on purpose"
                .into(),
        ));
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
        let procs = processes(&paths);
        out.push(AgentStatus {
            base_is_current: current.as_ref() == Some(&meta.base),
            worktree_exists: meta.worktree.is_dir(),
            private_bytes,
            // An error (e.g. an overlay lock held by unattributable processes)
            // still means something is using the agent.
            busy: !matches!(procs, Ok(None)),
            running: procs.ok().flatten().unwrap_or_default(),
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

    if let Some(pids) = processes(&paths)?
        && !force
    {
        return Err(Error::AgentBusy {
            name: name.to_string(),
            pids: if pids.is_empty() {
                "not attributable".into()
            } else {
                pids.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        });
    }
    if meta.worktree.is_dir() {
        let (changes, copied_lockfile) = user_changes(ctx, &meta)?;
        if !force && !changes.is_empty() {
            return Err(Error::Refused(format!(
                "{} has uncommitted changes (e.g. {}); commit them or pass --force to discard them",
                meta.worktree.display(),
                changes[0]
            )));
        }
        // `git worktree remove` refuses untracked files; this one is ours.
        if let Some(lockfile) = copied_lockfile {
            fs::remove_file(&lockfile).at("remove", &lockfile)?;
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
        let Some(agent_lock) = FileLock::try_exclusive(&paths.lock)? else {
            continue;
        };
        if processes(&paths)?.is_some()
            || (meta.worktree.is_dir() && !user_changes(ctx, &meta)?.0.is_empty())
            || ctx
                .repo
                .branch_tip(&meta.branch)?
                .is_some_and(|tip| tip != meta.from)
        {
            continue;
        }
        drop(agent_lock);
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
    let shim_dir = shim::ensure(&ctx.settings)?;
    enter(ctx, meta, &plan)?;
    hold_busy_lock(&ctx.store.agent(&meta.name))?;
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
    hold_busy_lock(&ctx.store.agent(&meta.name))?;
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
        shim_dir: shim::ensure(&ctx.settings)?,
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

/// Holds a shared lock on the agent's `busy.lock` for the rest of this
/// process and everything it execs.
///
/// The descriptor is deliberately leaked and made inheritable (std opens
/// files close-on-exec), so the lock stays held exactly as long as any
/// process started for the agent is alive. That lets `rm`, `list` and `gc`
/// tell whether an agent is in use on every backend and platform.
fn hold_busy_lock(paths: &AgentPaths) -> Result<()> {
    use std::os::fd::IntoRawFd;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&paths.busy_lock)
        .at("open", &paths.busy_lock)?;
    rustix::fs::flock(&file, rustix::fs::FlockOperation::LockShared)
        .map_err(std::io::Error::from)
        .at("lock", &paths.busy_lock)?;
    rustix::io::fcntl_setfd(&file, rustix::io::FdFlags::empty())
        .map_err(std::io::Error::from)
        .at("make inheritable", &paths.busy_lock)?;
    let _ = file.into_raw_fd();
    Ok(())
}

/// `None` if nothing runs as the agent, else the pids that can be
/// attributed (possibly none).
pub fn processes(paths: &AgentPaths) -> Result<Option<Vec<u32>>> {
    let mut pids = sandbox::users(&paths.ns_record)?;
    let busy = paths.busy_lock.exists() && FileLock::try_exclusive(&paths.busy_lock)?.is_none();
    if busy {
        pids.extend(sandbox::lock_holders(&paths.busy_lock));
    }
    pids.sort_unstable();
    pids.dedup();
    Ok((busy || !pids.is_empty()).then_some(pids))
}

/// The worktree's `Cargo.lock` if it is the copy spawn made from the base
/// (untracked and byte-identical to the base's), which is not the agent's
/// work.
fn copied_lockfile(ctx: &Context, meta: &AgentMeta, untracked: &[String]) -> Option<PathBuf> {
    if !untracked.iter().any(|p| p == "Cargo.lock") {
        return None;
    }
    let ours = meta.worktree.join("Cargo.lock");
    let base = ctx.store.base(&meta.base).src.join("Cargo.lock");
    match (fs::read(&ours), fs::read(&base)) {
        (Ok(a), Ok(b)) if a == b => Some(ours),
        _ => None,
    }
}

/// Uncommitted changes in the agent's worktree, ignoring the lockfile copy
/// spawn put there, and that copy's path if it is present.
fn user_changes(ctx: &Context, meta: &AgentMeta) -> Result<(Vec<String>, Option<PathBuf>)> {
    let entries = git::status_entries(&meta.worktree)?;
    let untracked: Vec<String> = entries
        .iter()
        .filter(|(code, _)| code == "??")
        .map(|(_, p)| p.clone())
        .collect();
    let copied = copied_lockfile(ctx, meta, &untracked);
    let changes = entries
        .into_iter()
        .filter(|(code, p)| !(copied.is_some() && code == "??" && p == "Cargo.lock"))
        .map(|(_, p)| p)
        .collect();
    Ok((changes, copied))
}
