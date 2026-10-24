//! Building, listing and collecting immutable warm bases.
//!
//! INVARIANT: once `base.json` exists, nothing writes to that base again.
//! Agents fork from it read-only, overlay agents use its target dir as their
//! lower layer, and a refresh builds a new base next to it instead of
//! updating it. `gc` deletes a base only when no agent references it and it
//! is not current.

use std::collections::BTreeSet;
use std::fs;
use std::os::fd::AsFd;
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::config::SandboxMode;
use crate::context::Context;
use crate::error::{Error, IoContext, Result};
use crate::fork::TargetSpec;
use crate::freshness::{self, FreshnessMode};
use crate::fsutil;
use crate::git::{self, CommitId};
use crate::lock::FileLock;
use crate::platform::Capabilities;
use crate::sandbox::{self, Isolation, Plan};
use crate::store::{BaseId, BasePaths, fnv1a64};
use crate::toolchain::Toolchain;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaseMeta {
    pub id: BaseId,
    pub commit: CommitId,
    pub created_unix: u64,
    pub build_secs: f64,
    pub freshness: FreshnessMode,
    pub isolation: Isolation,
    pub toolchain: Toolchain,
    pub commands: Vec<String>,
    pub ferry_version: String,
    /// Paths the warm commands created or modified inside the base checkout
    /// (build scripts writing outside `OUT_DIR`). Agents do not get these
    /// files, so they are reported as a warning.
    pub dirtied: Vec<String>,
    /// Built through the experimental `ferry-rustc` path-remapping wrapper;
    /// agents must use it too or rustc discards the incremental state.
    #[serde(default)]
    pub remap_paths: bool,
}

#[derive(Debug, Serialize)]
pub struct BuildReport {
    pub base: BaseMeta,
    /// True if an identical base already existed and was reused.
    pub reused: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BaseStatus {
    pub id: BaseId,
    pub ready: bool,
    pub current: bool,
    pub meta: Option<BaseMeta>,
    /// Agents forked from this base.
    pub agents: Vec<String>,
    pub disk_bytes: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct GcReport {
    pub removed: Vec<BaseId>,
    /// Bases skipped because another process is building them.
    pub busy: Vec<BaseId>,
    pub kept: Vec<BaseId>,
    /// Abandoned hook-created agents that were removed first.
    pub reaped_agents: Vec<String>,
    pub freed_bytes: u64,
    pub dry_run: bool,
}

/// Builds (or reuses) the base for `rev` and makes it current.
///
/// The calling process enters the base's namespace sandbox to run the warm
/// commands; that is irreversible, so call this at most once per process.
pub fn build(ctx: &Context, rev: &str) -> Result<BuildReport> {
    let mut warnings = Vec::new();
    let commit = ctx.repo.resolve_commit(rev)?;
    let host_toolchain = Toolchain::detect(&ctx.repo.main_worktree)?;
    let freshness = freshness::resolve_mode(ctx.settings.freshness, &host_toolchain)?;
    let isolation = resolve_isolation(ctx, &mut warnings)?;
    let remap = isolation == Isolation::None && ctx.settings.remap_paths && {
        match crate::remap::supported(&host_toolchain) {
            Ok(()) => true,
            Err(why) => {
                warnings.push(format!("remap-paths disabled: {why}"));
                false
            }
        }
    };
    let id = BaseId::new(
        commit.as_str(),
        fingerprint(ctx, &commit, &host_toolchain, freshness, isolation, remap)?,
    );
    let paths = ctx.store.base(&id);

    let lock = FileLock::exclusive(&paths.lock, &format!("base {}", id.short()))?;
    if let Some(base) = load_ready(&paths)? {
        set_current(ctx, &id)?;
        return Ok(BuildReport {
            base,
            reused: true,
            warnings,
        });
    }
    discard(ctx, &paths)?;

    let started = Instant::now();
    fs::create_dir_all(&paths.dir).at("create directory", &paths.dir)?;
    ctx.repo.add_detached_worktree(&paths.src, &commit)?;
    ctx.repo.lock_worktree(
        &paths.src,
        &format!("ferry base {} (immutable)", id.short()),
    )?;
    let index = git::ls_files(&paths.src)?;
    freshness::stamp_base_checkout(&paths.src, &index)?;
    let toolchain = Toolchain::detect(&paths.src)?;
    fs::create_dir_all(&paths.target).at("create directory", &paths.target)?;

    let remap_wrapper = if remap {
        crate::remap::prepare(&paths.src, &paths.target)?;
        Some(crate::remap::ensure_wrapper(ctx)?)
    } else {
        None
    };
    let plan = build_plan(ctx, &paths, isolation, remap_wrapper);
    let commands = ctx.settings.warm_commands.clone();
    if let Err(e) = run_warm_commands(&plan, freshness, &commands) {
        // The process may now be inside the sandbox, but host paths are
        // still reachable, so cleanup works the same way.
        let _ = discard(ctx, &paths);
        return Err(e);
    }

    let dirtied = git::status_paths(&paths.src)?;
    if !dirtied.is_empty() {
        warnings.push(format!(
            "the warm commands wrote {} file(s) into the source tree (e.g. {}); agents will not have them",
            dirtied.len(),
            dirtied[0]
        ));
    }
    let base = BaseMeta {
        id: id.clone(),
        commit,
        created_unix: fsutil::now_unix(),
        build_secs: started.elapsed().as_secs_f64(),
        freshness,
        isolation,
        toolchain,
        commands,
        ferry_version: env!("CARGO_PKG_VERSION").to_owned(),
        dirtied,
        remap_paths: remap,
    };
    // Writing base.json is what makes the base ready (and immutable).
    fsutil::write_json_atomic(&paths.meta, &base)?;
    drop(lock);
    set_current(ctx, &id)?;
    Ok(BuildReport {
        base,
        reused: false,
        warnings,
    })
}

/// Metadata of a complete base, or `None` if it does not exist or never
/// finished building.
pub fn load_ready(paths: &BasePaths) -> Result<Option<BaseMeta>> {
    if !paths.meta.exists() {
        return Ok(None);
    }
    fsutil::read_json(&paths.meta).map(Some)
}

pub fn load(ctx: &Context, id: &BaseId) -> Result<BaseMeta> {
    load_ready(&ctx.store.base(id))?.ok_or_else(|| Error::BaseMissing { id: id.to_string() })
}

pub fn current(ctx: &Context) -> Result<Option<BaseMeta>> {
    match ctx.store.current_base()? {
        Some(id) => load_ready(&ctx.store.base(&id)),
        None => Ok(None),
    }
}

/// Every ready base, newest first.
pub fn ready_bases(ctx: &Context) -> Result<Vec<BaseMeta>> {
    let mut bases = Vec::new();
    for id in ctx.store.base_ids()? {
        if let Some(meta) = load_ready(&ctx.store.base(&id))? {
            bases.push(meta);
        }
    }
    bases.sort_by(|a, b| b.created_unix.cmp(&a.created_unix));
    Ok(bases)
}

pub fn list(ctx: &Context) -> Result<Vec<BaseStatus>> {
    let current = ctx.store.current_base()?;
    let agents = crate::agent::all_meta(ctx)?;
    let mut out = Vec::new();
    for id in ctx.store.base_ids()? {
        let paths = ctx.store.base(&id);
        let meta = load_ready(&paths)?;
        out.push(BaseStatus {
            ready: meta.is_some(),
            current: current.as_ref() == Some(&id),
            agents: agents
                .iter()
                .filter(|a| a.base == id)
                .map(|a| a.name.to_string())
                .collect(),
            disk_bytes: fsutil::disk_usage(&paths.dir)?,
            meta,
            id,
        });
    }
    out.sort_by(|a, b| {
        let created = |s: &BaseStatus| s.meta.as_ref().map_or(0, |m| m.created_unix);
        created(b).cmp(&created(a))
    });
    Ok(out)
}

/// Removes abandoned hook-created agents (see
/// [`crate::agent::reap_abandoned`]), then deletes bases that are neither
/// current nor referenced by any agent, including incomplete ones left by
/// interrupted builds.
pub fn gc(ctx: &Context, dry_run: bool) -> Result<GcReport> {
    let _store = ctx.store.lock()?;
    let reaped = crate::agent::reap_abandoned(ctx, dry_run)?;
    let current = ctx.store.current_base()?;
    let referenced: BTreeSet<BaseId> = crate::agent::all_meta(ctx)?
        .into_iter()
        .filter(|a| !reaped.contains(&a.name))
        .map(|a| a.base)
        .collect();
    let mut report = GcReport {
        dry_run,
        reaped_agents: reaped.iter().map(ToString::to_string).collect(),
        ..GcReport::default()
    };
    for id in ctx.store.base_ids()? {
        if current.as_ref() == Some(&id) || referenced.contains(&id) {
            report.kept.push(id);
            continue;
        }
        let paths = ctx.store.base(&id);
        let Some(_building) = FileLock::try_exclusive(&paths.lock)? else {
            report.busy.push(id);
            continue;
        };
        report.freed_bytes += fsutil::disk_usage(&paths.dir)?;
        if !dry_run {
            discard(ctx, &paths)?;
        }
        report.removed.push(id);
    }
    if !dry_run {
        ctx.repo.prune_worktrees()?;
    }
    Ok(report)
}

fn set_current(ctx: &Context, id: &BaseId) -> Result<()> {
    let _store = ctx.store.lock()?;
    ctx.store.set_current_base(id)
}

/// Removes a base's worktree registration and files. Callers must hold the
/// base's lock and know that nothing references it.
fn discard(ctx: &Context, paths: &BasePaths) -> Result<()> {
    if paths.src.exists() && ctx.repo.remove_worktree(&paths.src, true).is_err() {
        // A half-created worktree may not be registered; deleting the files
        // and pruning below covers it.
    }
    fsutil::remove_dir_all_force(&paths.dir)?;
    ctx.repo.prune_worktrees()?;
    Ok(())
}

fn resolve_isolation(ctx: &Context, warnings: &mut Vec<String>) -> Result<Isolation> {
    if ctx.settings.sandbox == SandboxMode::Off {
        return Ok(Isolation::None);
    }
    let caps = Capabilities::detect(ctx.store.root());
    if caps.userns.is_ok() {
        return Ok(Isolation::Namespace);
    }
    let detail = match &caps.userns {
        crate::platform::Probe::Ok => String::new(),
        crate::platform::Probe::Unsupported { reason } => reason.clone(),
        crate::platform::Probe::Failed { stage, error } => format!("{stage}: {error}"),
    };
    if ctx.settings.sandbox == SandboxMode::Namespace {
        return Err(Error::Sandbox(sandbox::SandboxError::Unsupported(detail)));
    }
    if caps.os == crate::platform::Os::Linux {
        warnings.push(format!(
            "namespace sandbox unavailable ({detail}); building without canonical paths, so an agent's \
             first edit rebuilds without incremental reuse. Run `ferry doctor` for fixes."
        ));
    }
    Ok(Isolation::None)
}

/// Hash of everything besides the commit that determines a base's artifacts.
fn fingerprint(
    ctx: &Context,
    commit: &CommitId,
    toolchain: &Toolchain,
    freshness: FreshnessMode,
    isolation: Isolation,
    remap: bool,
) -> Result<u64> {
    let mut input = Vec::new();
    input.extend_from_slice(toolchain.rustc_verbose.as_bytes());
    input.push(0);
    input.extend_from_slice(toolchain.cargo_version.as_bytes());
    input.push(0);
    // The host toolchain only approximates the one the commit selects, so the
    // commit's own toolchain file is hashed too.
    for file in ["rust-toolchain.toml", "rust-toolchain"] {
        if let Some(bytes) = ctx.repo.show_file(commit, file)? {
            input.extend_from_slice(&bytes);
        }
        input.push(0);
    }
    for cmd in &ctx.settings.warm_commands {
        input.extend_from_slice(cmd.as_bytes());
        input.push(0);
    }
    input.extend_from_slice(format!("{freshness:?}/{isolation:?}/remap={remap}").as_bytes());
    Ok(fnv1a64(&input))
}

fn build_plan(
    ctx: &Context,
    paths: &BasePaths,
    isolation: Isolation,
    remap_wrapper: Option<std::path::PathBuf>,
) -> Plan {
    Plan {
        isolation,
        src: paths.src.clone(),
        target: TargetSpec::Dir(paths.target.clone()),
        canonical_src: ctx.store.canonical_src(),
        canonical_target: ctx.store.canonical_target(),
        ns_record: None,
        remap_wrapper,
    }
}

fn run_warm_commands(plan: &Plan, freshness: FreshnessMode, commands: &[String]) -> Result<()> {
    sandbox::enter(plan)?;
    let env = crate::agent::cargo_env(plan, freshness);
    for cmd in commands {
        eprintln!("ferry: running `{cmd}`");
        // Build output goes to stderr so `--json` output on stdout stays
        // parseable.
        let stderr = std::io::stderr()
            .as_fd()
            .try_clone_to_owned()
            .at("duplicate", "stderr")?;
        let status = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(plan.build_src())
            .env("PWD", plan.build_src())
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::from(stderr))
            .status()
            .map_err(|source| sandbox::SandboxError::Exec {
                program: "sh".into(),
                source,
            })?;
        if !status.success() {
            return Err(Error::BaseBuildFailed {
                command: cmd.clone(),
                status: status.to_string(),
            });
        }
    }
    Ok(())
}
