//! The fixture workspace and the harness, with plain cargo only. These pin
//! down the rebuild sets that every ferry test relies on.

mod common;

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::Command;

use common::{ALL_PACKAGES, App, BASE_MARKER, Build, Snapshot, TestRepo};

/// Plain cargo in the main checkout, with its target dir in scratch space.
fn cargo(repo: &TestRepo) -> Command {
    let mut cmd = repo.cargo_in(repo.root());
    cmd.env("CARGO_TARGET_DIR", repo.scratch().join("target"));
    cmd
}

#[test]
fn fixture_builds_offline_and_leaves_the_checkout_clean() {
    let repo = TestRepo::new();
    Build::run(cargo(&repo).args(["build", "--locked"])).assert_rebuilt(ALL_PACKAGES);
    Build::run(cargo(&repo).arg("build")).assert_rebuilt(&[]);
    assert_eq!(App::run(&mut cargo(&repo)), App::base());
    common::run(cargo(&repo).args(["test", "--workspace", "-q"])).ok();
    assert_eq!(repo.git(repo.root(), ["status", "--porcelain"]), "");
    assert_eq!(
        fs::read_to_string(repo.root().join("leaf/src/lib.rs")).unwrap(),
        common::leaf_source(BASE_MARKER),
        "set_marker() must produce the fixture's own leaf source"
    );
}

#[test]
fn edits_rebuild_exactly_the_crates_that_depend_on_them() {
    let repo = TestRepo::new();
    let root = repo.root();
    Build::run(cargo(&repo).arg("build")).assert_rebuilt(ALL_PACKAGES);

    repo.set_marker(root, "EDITED");
    Build::run(cargo(&repo).arg("build")).assert_rebuilt(&["leaf", "mid", "app"]);

    let side = root.join("side/src/lib.rs");
    let source = fs::read_to_string(&side).unwrap();
    repo.write(&side, &format!("{source}\npub fn extra() {{}}\n"));
    Build::run(cargo(&repo).arg("build")).assert_rebuilt(&["side"]);

    repo.set_asset(root, "a.txt", "changed");
    Build::run(cargo(&repo).arg("build")).assert_rebuilt(&["embed", "app"]);

    // Only the parent directory's mtime records a deletion.
    common::tick(repo.scratch());
    fs::remove_file(root.join("embed/assets/sub/c.txt")).unwrap();
    Build::run(cargo(&repo).arg("build")).assert_rebuilt(&["embed", "app"]);

    assert_eq!(
        App::run(&mut cargo(&repo)),
        App::new("EDITED", "a.txt=changed;sub/b.txt=beta")
    );
}

#[test]
fn snapshots_detect_changes_that_keep_the_size_or_the_mtime() {
    let repo = TestRepo::new();
    let dir = repo.root().join("embed/assets");
    let file = dir.join("a.txt");
    let detects = |before: &Snapshot| {
        catch_unwind(AssertUnwindSafe(|| {
            before.assert_unchanged(&Snapshot::of(&dir));
        }))
        .is_err()
    };

    let before = Snapshot::of(&dir);
    assert!(!detects(&before));
    repo.write(&file, "alpha\n");
    assert!(detects(&before), "same bytes, new mtime");

    let before = Snapshot::of(&dir);
    let mtime = fs::metadata(&file).unwrap().modified().unwrap();
    fs::write(&file, "ALPHA\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    assert!(detects(&before), "same size and mtime, new bytes");

    let before = Snapshot::of(&dir);
    fs::write(dir.join("sub/new.txt"), "").unwrap();
    assert!(detects(&before), "new file");
}

#[test]
fn package_names_come_from_package_ids() {
    assert_eq!(common::package_name("path+file:///ws/leaf#0.1.0"), "leaf");
    assert_eq!(
        common::package_name("path+file:///ws/crates/x#leaf@0.1.0"),
        "leaf"
    );
    assert_eq!(
        common::package_name("registry+https://github.com/rust-lang/crates.io-index#serde@1.0.228"),
        "serde"
    );
}

#[test]
fn writes_get_strictly_increasing_mtimes() {
    let repo = TestRepo::new();
    let first = repo.scratch().join("first");
    let second = repo.scratch().join("second");
    repo.write(&first, "");
    repo.write(&second, "");
    let mtime = |p| fs::metadata(p).unwrap().modified().unwrap();
    assert!(mtime(&second) > mtime(&first));
}
