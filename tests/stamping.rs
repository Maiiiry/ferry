//! Spawn gives a file the base's fixed mtime only when it is identical to
//! the file the base compiled, and a directory only when it lists the same
//! entries. Everything else keeps its fresh checkout mtime, so Cargo
//! rebuilds exactly what changed.

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::{TestRepo, is_stamped, path_of};

#[test]
fn spawn_stamps_tracked_files_and_their_directories() {
    let repo = TestRepo::new();
    repo.base();
    let worktree = repo.spawn("a");
    let mut dirs = BTreeSet::from([worktree.clone()]);
    for file in repo.git(&worktree, ["ls-files"]).lines() {
        let path = worktree.join(file);
        assert!(is_stamped(&path), "{file} is not stamped");
        dirs.extend(
            path.ancestors()
                .skip(1)
                .take_while(|d| d.starts_with(&worktree))
                .map(Path::to_path_buf),
        );
    }
    for dir in &dirs {
        assert!(is_stamped(dir), "{} is not stamped", dir.display());
    }
    assert!(dirs.contains(&worktree.join("embed/assets/sub")));
}

#[test]
fn a_file_that_differs_from_the_base_keeps_its_mtime() {
    let repo = TestRepo::new();
    repo.base();
    let side = repo.root().join("side/src/lib.rs");
    let source = fs::read_to_string(&side).unwrap();
    repo.write(&side, &format!("{source}\npub fn extra() {{}}\n"));
    repo.commit_all("change side");

    let spawned_at = SystemTime::now();
    let report = repo.ferry_json(["spawn", "a"]);
    let worktree = path_of(&report["agent"]["worktree"]);
    let stamp = &report["agent"]["stamp"];
    assert_eq!(stamp["skipped"], 1, "{report}");
    assert_eq!(
        stamp["stamped"].as_u64().unwrap() + 1,
        stamp["tracked"].as_u64().unwrap(),
        "{report}"
    );
    assert_ne!(report["warnings"], serde_json::json!([]), "{report}");

    let changed = worktree.join("side/src/lib.rs");
    assert!(!is_stamped(&changed));
    // File timestamps come from a coarse clock that may lag SystemTime.
    let mtime = fs::metadata(&changed).unwrap().modified().unwrap();
    assert!(mtime + Duration::from_secs(1) >= spawned_at);
    assert!(is_stamped(&worktree.join("side/src")));
    assert!(is_stamped(&worktree.join("leaf/src/lib.rs")));

    repo.build("a").assert_rebuilt(&["side"]);
    common::run(repo.agent_cargo("a").args(["test", "-q", "-p", "side"])).ok();
}

#[test]
fn a_deleted_file_leaves_its_directory_unstamped() {
    let repo = TestRepo::new();
    repo.base();
    repo.git(repo.root(), ["rm", "-q", "embed/assets/sub/c.txt"]);
    repo.commit_all("delete c.txt");
    let worktree = repo.spawn("a");

    let assets = worktree.join("embed/assets");
    assert!(!is_stamped(&assets.join("sub")));
    for unchanged in [
        assets.clone(),
        assets.join("a.txt"),
        assets.join("sub/b.txt"),
    ] {
        assert!(is_stamped(&unchanged), "{}", unchanged.display());
    }

    // The deletion is visible to the build script only through sub/'s mtime.
    repo.build("a").assert_rebuilt(&["embed", "app"]);
    assert_eq!(repo.app("a").assets, "a.txt=alpha;sub/b.txt=beta");
}

#[test]
fn an_agent_can_start_from_an_older_commit_than_the_base() {
    let repo = TestRepo::new();
    let first = repo.git(repo.root(), ["rev-parse", "HEAD"]);
    repo.set_marker(repo.root(), "NEWER");
    repo.commit_all("newer marker");
    repo.base();

    let report = repo.ferry_json(["spawn", "a", "--from", &first]);
    let worktree = path_of(&report["agent"]["worktree"]);
    assert!(!is_stamped(&worktree.join("leaf/src/lib.rs")));
    repo.build("a").assert_rebuilt(&["leaf", "mid", "app"]);
    assert_eq!(repo.app("a").marker, common::BASE_MARKER);
}
