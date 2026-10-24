//! The repository, configuration and store a command operates on.

use std::path::{Path, PathBuf};

use crate::config::{Config, Settings};
use crate::error::{Error, IoContext, Result};
use crate::git::Repo;
use crate::store::{self, Store};

#[derive(Debug, Clone)]
pub struct Context {
    pub repo: Repo,
    pub config: Config,
    pub settings: Settings,
    pub store: Store,
}

impl Context {
    /// Discovers the repository containing `dir` and opens (creating if
    /// needed) its store, reading configuration from the process environment.
    pub fn discover(dir: &Path) -> Result<Self> {
        Self::discover_with_env(dir, &|k| std::env::var(k).ok())
    }

    pub fn discover_with_env(dir: &Path, env: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let (repo, config, settings, root) = resolve(dir, env)?;
        let store = Store::open(root, &repo.main_worktree)?;
        Ok(Self {
            repo,
            config,
            settings,
            store,
        })
    }

    /// Like [`Context::discover`], but `None` when the repository has no
    /// store yet. Used on paths that run for every cargo call (the shim) or
    /// every agent session, which must not leave a store behind in each
    /// repository they pass through.
    pub fn discover_existing(dir: &Path) -> Result<Option<Self>> {
        let env = |k: &str| std::env::var(k).ok();
        let (repo, config, settings, root) = resolve(dir, &env)?;
        if !root.join("store.json").is_file() {
            return Ok(None);
        }
        let store = Store::open(root, &repo.main_worktree)?;
        Ok(Some(Self {
            repo,
            config,
            settings,
            store,
        }))
    }
}

/// Settings for `dir`'s repository without touching the store, e.g. to find
/// the shim directory.
pub fn settings(dir: &Path) -> Result<Settings> {
    let env = |k: &str| std::env::var(k).ok();
    Ok(resolve(dir, &env)?.2)
}

fn resolve(
    dir: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(Repo, Config, Settings, PathBuf)> {
    let repo = Repo::discover(dir)?;
    let (config, config_path) = Config::load(&repo.main_worktree, env)?;
    let settings = config.resolve(&repo.main_worktree, config_path, env)?;
    let parent = settings.store_parent.clone().ok_or(Error::NoStoreDir)?;
    // Canonical so that reaching the repository through a symlink still
    // finds the same store.
    let common_dir = std::fs::canonicalize(&repo.common_dir).at("resolve", &repo.common_dir)?;
    let name = repo
        .main_worktree
        .file_name()
        .map_or_else(|| "repo".into(), |n| n.to_string_lossy().into_owned());
    let root = parent.join(store::repo_key(&common_dir, &name));
    Ok((repo, config, settings, root))
}
