//! The repository, configuration and store a command operates on.

use std::path::Path;

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
    /// Discovers the repository containing `dir` and opens its store, reading
    /// configuration from the process environment.
    pub fn discover(dir: &Path) -> Result<Self> {
        Self::discover_with_env(dir, &|k| std::env::var(k).ok())
    }

    pub fn discover_with_env(dir: &Path, env: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
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
        let store = Store::open(
            parent.join(store::repo_key(&common_dir, &name)),
            &repo.main_worktree,
        )?;
        Ok(Self {
            repo,
            config,
            settings,
            store,
        })
    }
}
