//! Bases are immutable once built, and a refreshed base never disturbs the
//! agents still using an older one.

mod common;

use common::{BASE_MARKER, Snapshot, TestRepo, str_of};
use serde_json::Value;

fn current_base(list: &Value) -> &str {
    let current: Vec<&Value> = list["bases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["current"] == true)
        .collect();
    assert_eq!(current.len(), 1, "{list}");
    str_of(&current[0]["id"])
}

#[test]
fn agents_never_modify_the_base() {
    let repo = TestRepo::new();
    let base = repo.base();
    let dir = repo.base_dir(&base);
    let before = Snapshot::of(&dir);

    let worktree = repo.spawn("a");
    repo.build("a").assert_rebuilt(&[]);
    repo.set_marker(&worktree, "AGENT_EDIT");
    repo.set_asset(&worktree, "a.txt", "agent");
    repo.build("a")
        .assert_rebuilt(&["leaf", "mid", "embed", "app"]);
    common::run(repo.agent_cargo("a").args(["test", "--workspace", "-q"])).ok();
    common::run(repo.agent_cargo("a").args(["clean", "-p", "leaf"])).ok();
    repo.build("a").assert_rebuilt(&["leaf", "mid", "app"]);
    before.assert_unchanged(&Snapshot::of(&dir));

    repo.ferry(["rm", "--force", "a"]).ok();
    before.assert_unchanged(&Snapshot::of(&dir));
}

#[test]
fn building_the_same_commit_again_reuses_the_base() {
    let repo = TestRepo::new();
    let first = repo.ferry_json(["base"]);
    assert_eq!(first["reused"], false, "{first}");
    let second = repo.ferry_json(["base"]);
    assert_eq!(second["reused"], true, "{second}");
    assert_eq!(first["base"]["id"], second["base"]["id"]);

    let list = repo.ferry_json(["list"]);
    assert_eq!(list["bases"].as_array().map(Vec::len), Some(1), "{list}");
    assert_eq!(list["bases"][0]["ready"], true, "{list}");
    assert_eq!(current_base(&list), str_of(&first["base"]["id"]));
}

#[test]
fn a_new_base_leaves_live_agents_on_the_old_one_until_they_are_removed() {
    let repo = TestRepo::new();
    let first_commit = repo.git(repo.root(), ["rev-parse", "HEAD"]);
    let base1 = repo.base();
    repo.spawn("old");

    repo.set_marker(repo.root(), "LEAF_V2");
    repo.commit_all("v2");
    let base2 = repo.base();
    assert_ne!(base1, base2);
    let list = repo.ferry_json(["list"]);
    assert_eq!(current_base(&list), base2);
    let old = &list["agents"][0];
    assert_eq!(old["name"], "old", "{list}");
    assert_eq!(old["base"], base1.as_str(), "{list}");
    assert_eq!(old["base_is_current"], false, "{list}");

    repo.build("old").assert_rebuilt(&[]);
    assert_eq!(repo.app("old").marker, BASE_MARKER);

    repo.spawn("new");
    repo.build("new").assert_rebuilt(&[]);
    assert_eq!(repo.app("new").marker, "LEAF_V2");
    // A ready base built from exactly the requested commit wins over the
    // current one.
    let pinned = repo.ferry_json(["spawn", "pinned", "--from", &first_commit]);
    assert_eq!(pinned["agent"]["base"], base1.as_str(), "{pinned}");

    let gc = repo.ferry_json(["gc"]);
    assert_eq!(gc["removed"], serde_json::json!([]), "{gc}");
    assert!(repo.base_dir(&base1).is_dir());

    repo.ferry(["rm", "old"]).ok();
    repo.ferry(["rm", "pinned"]).ok();
    let dry = repo.ferry_json(["gc", "--dry-run"]);
    assert_eq!(dry["removed"], serde_json::json!([base1]), "{dry}");
    assert!(repo.base_dir(&base1).is_dir(), "--dry-run deleted the base");

    let gc = repo.ferry_json(["gc"]);
    assert_eq!(gc["removed"], serde_json::json!([base1]), "{gc}");
    assert!(!repo.base_dir(&base1).exists());
    assert!(repo.base_dir(&base2).is_dir());
    let worktrees = repo.git(repo.root(), ["worktree", "list", "--porcelain"]);
    assert!(!worktrees.contains(&base1), "{worktrees}");

    repo.build("new").assert_rebuilt(&[]);
}

/// Package names cargo compiled (not fresh) according to the JSON messages
/// a warm command printed; `ferry base` sends build output to stderr.
fn compiled_in_base_build(stderr: &str) -> Vec<String> {
    let mut names: Vec<String> = stderr
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["fresh"] == false)
        .map(|m| common::package_name(str_of(&m["package_id"])).to_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn a_refreshed_base_is_seeded_from_the_previous_one_and_rebuilds_only_changes() {
    let json_build = [(
        "FERRY_BASE_COMMAND",
        "cargo build --workspace --message-format=json",
    )];
    let repo = TestRepo::with_env(&json_build);
    let base1 = repo.base();
    let before = Snapshot::of(&repo.base_dir(&base1));

    repo.set_marker(repo.root(), "LEAF_V2");
    repo.commit_all("v2");
    let refresh = repo.ferry(["--json", "base"]).ok();
    let report = refresh.json();
    assert_eq!(report["base"]["seeded_from"], base1.as_str(), "{report}");
    assert_eq!(
        compiled_in_base_build(&refresh.stderr),
        ["app", "leaf", "mid"]
    );
    before.assert_unchanged(&Snapshot::of(&repo.base_dir(&base1)));

    repo.spawn("a");
    repo.build("a").assert_rebuilt(&[]);
    assert_eq!(repo.app("a").marker, "LEAF_V2");

    repo.set_marker(repo.root(), "LEAF_V3");
    repo.commit_all("v3");
    let cold = repo.ferry(["--json", "base", "--cold"]).ok();
    assert_eq!(cold.json()["base"]["seeded_from"], Value::Null);
    assert!(compiled_in_base_build(&cold.stderr).len() >= common::ALL_PACKAGES.len());
}

/// A seeded target also holds units from the seed's own seeds. When a
/// manifest change selects one of those again (here a default feature turned
/// off and back on), it was built from older sources than the ones stamping
/// compared against, so the seed must not be used.
#[test]
fn a_base_is_not_seeded_across_manifest_changes() {
    let repo = TestRepo::new();
    let manifest = repo.root().join("leaf/Cargo.toml");
    let plain = std::fs::read_to_string(&manifest).unwrap();
    let with_feature = format!("{plain}\n[features]\ndefault = [\"x\"]\nx = []\n");

    repo.write(&manifest, &with_feature);
    repo.set_marker(repo.root(), "LEAF_A");
    repo.commit_all("A: feature on");
    let a = repo.base();

    repo.write(&manifest, &plain);
    repo.set_marker(repo.root(), "LEAF_B");
    repo.commit_all("B: feature off");
    // So that B, were it eligible, would be the newest seed for C.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let b = repo.ferry_json(["base"]);
    assert_eq!(b["base"]["seeded_from"], Value::Null, "{b}");

    repo.write(&manifest, &with_feature);
    repo.commit_all("C: feature on again");
    let c = repo.ferry_json(["base"]);
    assert_eq!(c["base"]["seeded_from"], a.as_str(), "{c}");

    repo.spawn("agent");
    assert_eq!(repo.app("agent").marker, "LEAF_B");
}
