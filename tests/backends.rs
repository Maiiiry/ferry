//! The core scenario under every fork backend and sandbox mode the machine
//! supports. Backends that are unavailable here are skipped with a message.

mod common;

use common::{ALL_PACKAGES, App, Snapshot, TestRepo, path_of};

fn scenario(backend: &str, sandbox: &str) {
    let repo = TestRepo::with_env(&[("FERRY_BACKEND", backend), ("FERRY_SANDBOX", sandbox)]);
    let available = match backend {
        "reflink" => repo.require_reflink(),
        "overlay" => repo.require_overlay(),
        _ => true,
    };
    if !available {
        return;
    }
    let isolation = if sandbox != "off" && repo.capabilities().userns.is_ok() {
        "namespace"
    } else {
        "none"
    };
    let base = repo.base();
    let before = Snapshot::of(&repo.base_dir(&base));

    let report = repo.ferry_json(["spawn", "a"]);
    assert_eq!(report["agent"]["backend"], backend, "{report}");
    assert_eq!(report["agent"]["isolation"], isolation, "{report}");
    let worktree = path_of(&report["agent"]["worktree"]);
    // At its own paths the agent cannot trust local units, which may have
    // captured the base's; dependencies would still be reused.
    let relocated: &[&str] = if isolation == "none" {
        ALL_PACKAGES
    } else {
        &[]
    };
    repo.build("a").assert_rebuilt(relocated);
    assert_eq!(repo.app("a"), App::base());

    repo.set_marker(&worktree, "EDITED");
    repo.build("a").assert_rebuilt(&["leaf", "mid", "app"]);
    assert_eq!(repo.app("a").marker, "EDITED");

    repo.ferry(["rm", "--force", "a"]).ok();
    before.assert_unchanged(&Snapshot::of(&repo.base_dir(&base)));
}

#[test]
fn copy_backend() {
    scenario("copy", "auto");
}

#[test]
fn copy_backend_without_sandbox() {
    scenario("copy", "off");
}

#[test]
fn reflink_backend() {
    scenario("reflink", "auto");
}

#[test]
fn reflink_backend_without_sandbox() {
    scenario("reflink", "off");
}

#[test]
fn overlay_backend() {
    scenario("overlay", "auto");
}

#[test]
fn overlay_without_a_sandbox_is_refused_and_rolled_back() {
    let repo = TestRepo::with_env(&[("FERRY_BACKEND", "overlay"), ("FERRY_SANDBOX", "off")]);
    repo.base();
    let refused = repo.ferry(["spawn", "a"]).failed();
    assert!(refused.stderr.contains("error:"), "{refused}");
    assert!(refused.stderr.contains("overlay"), "{refused}");

    assert_eq!(repo.ferry_json(["list"])["agents"], serde_json::json!([]));
    assert_eq!(repo.git(repo.root(), ["branch", "--list", "ferry/a"]), "");
    let worktrees = repo.git(repo.root(), ["worktree", "list", "--porcelain"]);
    assert_eq!(
        worktrees
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        2,
        "only the main checkout and the base should remain:\n{worktrees}"
    );
}
