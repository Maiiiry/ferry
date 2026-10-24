//! A spawned agent starts warm: nothing rebuilds until it edits, and then
//! only what depends on the edit.

mod common;

use common::{App, Build, TestRepo, path_of, str_of};

#[test]
fn spawn_without_edits_rebuilds_nothing() {
    let repo = TestRepo::new();
    repo.base();
    repo.spawn("a");
    repo.build("a").assert_rebuilt(&[]);
    assert_eq!(repo.app("a"), App::base());
}

#[test]
fn an_edit_rebuilds_only_the_edited_crate_and_its_dependents() {
    let repo = TestRepo::new();
    repo.base();
    let worktree = repo.spawn("a");
    repo.set_marker(&worktree, "EDITED");
    repo.build("a").assert_rebuilt(&["leaf", "mid", "app"]);
    assert_eq!(repo.app("a").marker, "EDITED");
    repo.build("a").assert_rebuilt(&[]);
}

#[test]
fn warm_test_targets_are_reused() {
    let repo = TestRepo::new();
    repo.base();
    repo.spawn("a");
    Build::run(
        repo.agent_cargo("a")
            .args(["test", "--workspace", "--no-run"]),
    )
    .assert_rebuilt(&[]);
    common::run(repo.agent_cargo("a").args(["test", "--workspace", "-q"])).ok();
}

/// macOS's default worktree directory (under `~/Library/Application
/// Support`) contains a space; the harness's own `FERRY_HOME` never does.
#[test]
fn spawn_and_build_in_a_worktree_path_with_spaces() {
    let repo = TestRepo::new();
    repo.base();
    let parent = repo.scratch().join("Application Support/wt");
    let mut spawn = repo.ferry_cmd(["--json", "spawn", "a"]);
    spawn.env("FERRY_WORKTREES", &parent);
    let worktree = path_of(&common::run(&mut spawn).ok().json()["agent"]["worktree"]);
    assert!(worktree.starts_with(&parent), "{}", worktree.display());
    repo.build("a").assert_rebuilt(&[]);
    repo.set_marker(&worktree, "EDITED");
    repo.build("a").assert_rebuilt(&["leaf", "mid", "app"]);
    assert_eq!(repo.app("a").marker, "EDITED");
    common::run(repo.agent_cargo("a").args(["test", "--workspace", "-q"])).ok();
}

#[test]
fn spawn_honours_path_and_branch_and_run_finds_the_agent_from_its_cwd() {
    let repo = TestRepo::new();
    repo.base();
    let path = repo.scratch().join("elsewhere/a");
    let report = repo.ferry_json([
        "spawn",
        "a",
        "--path",
        path.to_str().unwrap(),
        "--branch",
        "topic/a",
    ]);
    assert_eq!(path_of(&report["agent"]["worktree"]), path);
    assert_eq!(str_of(&report["agent"]["branch"]), "topic/a");
    assert_eq!(repo.git(&path, ["branch", "--show-current"]), "topic/a");

    let mut cargo = repo.ferry_cmd(["run", "--", "cargo", "build"]);
    cargo.current_dir(path.join("leaf"));
    Build::run(&mut cargo).assert_rebuilt(&[]);
}
