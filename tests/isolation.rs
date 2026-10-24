//! Where an agent's cargo runs and writes: canonical paths inside the
//! namespace sandbox, host paths without it, and never a directory chosen by
//! user configuration.

mod common;

use std::fs;

use common::{TestRepo, path_of};
use serde_json::Value;

fn cargo_metadata(repo: &TestRepo, agent: &str) -> Value {
    common::run(
        repo.agent_cargo(agent)
            .args(["metadata", "--format-version=1", "--no-deps"]),
    )
    .ok()
    .json()
}

#[test]
fn cargo_runs_at_the_canonical_paths_inside_the_namespace_sandbox() {
    let repo = TestRepo::new();
    if !repo.require_userns() {
        return;
    }
    repo.base();
    let worktree = repo.spawn("a");
    let mnt = repo.store().join("mnt");

    let metadata = cargo_metadata(&repo, "a");
    assert_eq!(path_of(&metadata["workspace_root"]), mnt.join("src"));
    assert_eq!(path_of(&metadata["target_directory"]), mnt.join("target"));

    // Other commands keep the host path, which agent tools key state on.
    let pwd = common::run(&mut repo.ferry_cmd(["run", "a", "--", "pwd"])).ok();
    assert_eq!(pwd.stdout.trim(), worktree.to_str().unwrap());

    // The mounts exist only inside the sandbox.
    for dir in ["src", "target"] {
        assert_eq!(fs::read_dir(mnt.join(dir)).unwrap().count(), 0, "mnt/{dir}");
    }
}

#[test]
fn cargo_runs_at_host_paths_without_a_sandbox() {
    let repo = TestRepo::with_env(&[("FERRY_SANDBOX", "off")]);
    repo.base();
    let worktree = repo.spawn("a");
    let env = repo.ferry_json(["env", "a"]);
    assert_eq!(env["isolation"], "none", "{env}");

    let metadata = cargo_metadata(&repo, "a");
    assert_eq!(path_of(&metadata["workspace_root"]), worktree);
    assert_eq!(
        path_of(&metadata["target_directory"]),
        path_of(&env["host_target_dir"])
    );
}

#[test]
fn configured_target_and_build_dirs_cannot_redirect_builds() {
    let repo = TestRepo::new();
    let redirected = repo.scratch().join("redirected");
    let config = format!(
        "[build]\ntarget-dir = \"{0}/target\"\nbuild-dir = \"{0}/build/{{workspace-path-hash}}\"\n",
        redirected.display()
    );
    fs::create_dir(repo.root().join(".cargo")).unwrap();
    repo.write(&repo.root().join(".cargo/config.toml"), &config);
    repo.commit_all("cargo config with shared dirs");

    repo.base();
    let worktree = repo.spawn("a");
    repo.build("a").assert_rebuilt(&[]);
    repo.set_marker(&worktree, "EDITED");
    repo.build("a").assert_rebuilt(&["leaf", "mid", "app"]);
    assert!(
        !redirected.exists(),
        "a build wrote to {}",
        redirected.display()
    );
}
