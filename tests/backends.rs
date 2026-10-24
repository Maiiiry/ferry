//! The core scenario under every fork backend and sandbox mode the machine
//! supports. Backends that are unavailable here are skipped with a message.

mod common;

use common::{App, Build, Snapshot, TestRepo, path_of};

/// What `auto` must pick here (`fork::select`).
fn expected_auto(repo: &TestRepo, sandbox: &str) -> &'static str {
    let caps = repo.capabilities();
    if caps.reflink.is_ok() {
        "reflink"
    } else if sandbox != "off" && caps.userns.is_ok() && caps.overlay.is_ok() {
        "overlay"
    } else {
        "copy"
    }
}

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
    let expected = if backend == "auto" {
        expected_auto(&repo, sandbox)
    } else {
        backend
    };
    let isolation = if sandbox != "off" && repo.capabilities().userns.is_ok() {
        "namespace"
    } else {
        "none"
    };
    let base = repo.base();
    let before = Snapshot::of(&repo.base_dir(&base));

    let doctor = common::run(&mut repo.ferry_cmd(["--json", "doctor"])).json();
    assert_eq!(doctor["plan"]["backend"], expected, "{doctor}");
    let report = repo.ferry_json(["spawn", "a"]);
    assert_eq!(report["agent"]["backend"], expected, "{report}");
    assert_eq!(report["agent"]["isolation"], isolation, "{report}");
    let worktree = path_of(&report["agent"]["worktree"]);
    // Even at its own paths the agent reuses every unit: none of the
    // fixture's captured the base's paths.
    repo.build("a").assert_rebuilt(&[]);
    assert_eq!(repo.app("a"), App::base());

    repo.set_marker(&worktree, "EDITED");
    repo.build("a").assert_rebuilt(&["leaf", "mid", "app"]);
    assert_eq!(repo.app("a").marker, "EDITED");

    repo.ferry(["rm", "--force", "a"]).ok();
    before.assert_unchanged(&Snapshot::of(&repo.base_dir(&base)));
}

#[test]
fn auto_backend() {
    scenario("auto", "auto");
}

#[test]
fn auto_backend_without_sandbox() {
    scenario("auto", "off");
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

/// The backend is recorded at spawn: an overlay agent keeps overlay after
/// `auto` starts preferring reflink, and still builds warm.
#[test]
fn an_overlay_agent_keeps_overlay_under_auto() {
    let repo = TestRepo::with_env(&[("FERRY_BACKEND", "overlay")]);
    if !repo.require_overlay() {
        return;
    }
    repo.base();
    repo.spawn("a");
    let mut env = repo.ferry_cmd(["--json", "env", "a"]);
    env.env("FERRY_BACKEND", "auto");
    let env = common::run(&mut env).ok().json();
    assert_eq!(env["backend"], "overlay", "{env}");
    assert!(env["host_target_dir"].is_null(), "{env}");
    let mut cargo = repo.agent_cargo("a");
    cargo.env("FERRY_BACKEND", "auto").arg("build");
    Build::run(&mut cargo).assert_rebuilt(&[]);
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
