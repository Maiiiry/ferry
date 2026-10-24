//! The bug Ferry exists to prevent. Cargo decides freshness by comparing
//! source mtimes with its own records, so when two worktrees share one
//! target dir, an older edit in one of them looks "fresh" next to the other's
//! newer build: B is handed A's code with nothing rebuilt.

mod common;

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use common::{ALL_PACKAGES, App, Build, TestRepo};

fn assets_with(a: &str) -> String {
    format!("a.txt={a};sub/b.txt=beta;sub/c.txt=gamma")
}

/// B edits first, then A edits differently. Both change the leaf marker (a
/// Rust source) and an asset (a build-script input).
fn edit_b_then_a(repo: &TestRepo, a: &Path, b: &Path) {
    repo.set_marker(b, "ONLY_IN_B");
    repo.set_asset(b, "a.txt", "a-from-B");
    repo.set_marker(a, "ONLY_IN_A");
    repo.set_asset(a, "a.txt", "a-from-A");
}

#[test]
fn naive_shared_target_dir_gives_one_worktree_the_others_code() {
    let repo = TestRepo::new();
    let a = repo.scratch().join("worktree-a");
    let b = repo.scratch().join("worktree-b");
    for (dir, branch) in [(&a, "a"), (&b, "b")] {
        let dir = dir.to_str().unwrap();
        repo.git(repo.root(), ["worktree", "add", "-q", "-b", branch, dir]);
    }
    let shared = repo.scratch().join("shared-target");
    let cargo = |dir: &Path| -> Command {
        let mut cmd = repo.cargo_in(dir);
        cmd.env("CARGO_TARGET_DIR", &shared);
        cmd
    };

    Build::run(cargo(&a).arg("build")).assert_rebuilt(ALL_PACKAGES);
    edit_b_then_a(&repo, &a, &b);
    Build::run(cargo(&a).arg("build")).assert_rebuilt(&["leaf", "mid", "embed", "app"]);
    assert_eq!(
        App::run(&mut cargo(&a)),
        App::new("ONLY_IN_A", &assets_with("a-from-A"))
    );

    // Every source in B, edited or not, is older than A's artifacts.
    Build::run(cargo(&b).arg("build")).assert_rebuilt(&[]);
    assert_eq!(
        App::run(&mut cargo(&b)),
        App::new("ONLY_IN_A", &assets_with("a-from-A")),
        "B ran A's code"
    );

    let b_source = fs::read_to_string(b.join("leaf/src/lib.rs")).unwrap();
    assert!(b_source.contains("ONLY_IN_B"));
    let rlib = leaf_rlib(&shared).unwrap();
    assert!(contains(&rlib, b"ONLY_IN_A"));
    assert!(!contains(&rlib, b"ONLY_IN_B"));
}

#[test]
fn ferry_agents_each_build_their_own_code() {
    let repo = TestRepo::new();
    repo.base();
    let a = repo.spawn("a");
    let b = repo.spawn("b");

    edit_b_then_a(&repo, &a, &b);
    repo.build("a")
        .assert_rebuilt(&["leaf", "mid", "embed", "app"]);
    repo.build("b")
        .assert_rebuilt(&["leaf", "mid", "embed", "app"]);
    assert_eq!(
        repo.app("a"),
        App::new("ONLY_IN_A", &assets_with("a-from-A"))
    );
    assert_eq!(
        repo.app("b"),
        App::new("ONLY_IN_B", &assets_with("a-from-B"))
    );

    // Neither agent's build invalidated the other's.
    repo.build("a").assert_rebuilt(&[]);
    repo.build("b").assert_rebuilt(&[]);
}

/// The single `libleaf-*.rlib` in a `cargo build` target dir.
fn leaf_rlib(target: &Path) -> io::Result<Vec<u8>> {
    let mut rlibs = Vec::new();
    for entry in fs::read_dir(target.join("debug/deps"))? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("libleaf-") && path.extension().is_some_and(|e| e == "rlib") {
            rlibs.push(path);
        }
    }
    assert_eq!(rlibs.len(), 1, "{rlibs:?}");
    fs::read(&rlibs[0])
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
