//! A spawned agent starts warm: nothing rebuilds until it edits, and then
//! only what depends on the edit.

mod common;

use std::path::Path;

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
fn an_agent_ahead_of_the_base_is_told_how_to_catch_up() {
    let repo = TestRepo::new();
    repo.base();
    repo.set_marker(repo.root(), "LEAF_V2");
    repo.commit_all("v2");
    let spawn = repo.ferry_json(["spawn", "a"]);
    let warnings = spawn["warnings"].to_string();
    assert!(warnings.contains("ferry base --watch"), "{spawn}");
    assert_eq!(repo.app("a").marker, "LEAF_V2");
}

/// `ferry base` and the watcher build HEAD, which would not help an agent
/// started from another branch.
#[test]
fn an_agent_from_another_branch_is_told_to_build_a_base_for_it() {
    let repo = TestRepo::new();
    repo.base();
    repo.git(repo.root(), ["checkout", "-q", "-b", "feature-x"]);
    repo.set_marker(repo.root(), "FEATURE_X");
    let feature = repo.commit_all("feature x");
    repo.git(repo.root(), ["checkout", "-q", "main"]);
    let spawn = repo.ferry_json(["spawn", "a", "--from", "feature-x"]);
    let warnings = spawn["warnings"].to_string();
    assert!(
        warnings.contains(&format!("`ferry base --rev {}`", &feature[..12])),
        "{spawn}"
    );
    assert!(!warnings.contains("--watch"), "{spawn}");
    assert_eq!(repo.app("a").marker, "FEATURE_X");
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
/// Without the sandbox, so that cargo runs at that path on Linux too
/// instead of at the namespace's canonical one.
#[test]
fn spawn_and_build_in_a_worktree_path_with_spaces() {
    let repo = TestRepo::with_env(&[("FERRY_SANDBOX", "off")]);
    repo.base();
    let parent = repo.scratch().join("Application Support/wt");
    let mut spawn = repo.ferry_cmd(["--json", "spawn", "a"]);
    spawn.env("FERRY_WORKTREES", &parent);
    let worktree = path_of(&common::run(&mut spawn).ok().json()["agent"]["worktree"]);
    assert!(worktree.starts_with(&parent), "{}", worktree.display());
    let manifest = common::run(repo.agent_cargo("a").args([
        "locate-project",
        "--workspace",
        "--message-format=plain",
    ]))
    .ok();
    assert_eq!(
        Path::new(manifest.stdout.trim()),
        worktree.join("Cargo.toml"),
        "cargo must run at the spaced path"
    );
    // At another path than the base's, the first build may rebuild the
    // local crates (see `freshness::relocate`); that is not what this tests.
    repo.build("a");
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
