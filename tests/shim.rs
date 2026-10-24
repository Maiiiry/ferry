//! The cargo shim decides per call, from the working directory alone,
//! whether cargo builds in an agent's context or runs untouched.

mod common;

use common::{ALL_PACKAGES, Build, TestRepo};

#[test]
fn shim_builds_in_the_agent_whose_worktree_holds_the_cwd() {
    let repo = TestRepo::new();
    repo.base();
    let worktree = repo.spawn("a");

    Build::run(repo.shimmed_cargo(&worktree).arg("build")).assert_rebuilt(&[]);
    Build::run(repo.shimmed_cargo(&worktree.join("leaf/src")).arg("build")).assert_rebuilt(&[]);
    assert!(!worktree.join("target").exists());

    // An inherited target dir (from a parent agent's environment, say) must
    // not redirect the agent's build.
    let inherited = repo.scratch().join("inherited-target");
    Build::run(
        repo.shimmed_cargo(&worktree)
            .env("CARGO_TARGET_DIR", &inherited)
            .arg("build"),
    )
    .assert_rebuilt(&[]);
    assert!(!inherited.exists());
}

#[test]
fn shim_runs_plain_cargo_outside_agent_worktrees() {
    let repo = TestRepo::new();
    repo.base();
    repo.spawn("a");

    Build::run(repo.shimmed_cargo(repo.root()).arg("build")).assert_rebuilt(ALL_PACKAGES);
    assert!(repo.root().join("target/debug/app").is_file());

    let outside = repo.scratch().join("not-a-repo");
    std::fs::create_dir(&outside).unwrap();
    common::run(repo.shimmed_cargo(&outside).arg("--version")).ok();
}
