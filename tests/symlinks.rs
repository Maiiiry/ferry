//! A tracked symlink that leads elsewhere in an agent than in its base must
//! not let Cargo keep the base's artifacts. Cargo records a rustc input read
//! through a symlink by its canonical path in the base, and follows a build
//! script's `rerun-if-changed` path through the link in the agent. Both
//! targets can be identical to the base's, so spawn leaves both unstamped.
//! Each test expects what a cold build of the agent's commit prints.

// clippy's allow-unwrap-in-tests covers only #[test] functions, not their
// helpers.
#![allow(clippy::unwrap_used)]

mod common;

use std::fs;
use std::os::unix::fs::symlink;

use common::{TestRepo, is_stamped, path_of};

const INCLUDE_CURRENT: &str =
    "pub fn marker() -> &'static str {\n    include_str!(\"../data/current.txt\")\n}\n";

const PATH_MODULE: &str = "#[path = \"../impl/marker.rs\"]\nmod imp;\n\n\
     pub fn marker() -> &'static str {\n    imp::MARKER\n}\n";

/// Writes regular files under `leaf/`, replacing any symlink in the way.
fn put(repo: &TestRepo, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let path = repo.root().join("leaf").join(path);
        if path.is_symlink() {
            fs::remove_file(&path).unwrap();
        }
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

/// Makes `leaf/<at>` a symlink to `target`, replacing what is there.
fn link(repo: &TestRepo, at: &str, target: &str) {
    let at = repo.root().join("leaf").join(at);
    if at.symlink_metadata().is_ok() {
        fs::remove_file(&at).unwrap();
    }
    symlink(target, at).unwrap();
}

fn set_leaf(repo: &TestRepo, source: &str) {
    repo.write(&repo.root().join("leaf/src/lib.rs"), source);
}

/// The base has `current.txt -> v1.txt`, the agent `current.txt -> v2.txt`.
fn retarget_current_txt(repo: &TestRepo) {
    put(repo, &[("data/v1.txt", "ONE"), ("data/v2.txt", "TWO")]);
    link(repo, "data/current.txt", "v1.txt");
    set_leaf(repo, INCLUDE_CURRENT);
    repo.commit_all("current.txt -> v1.txt");
    repo.base();
    link(repo, "data/current.txt", "v2.txt");
    repo.commit_all("current.txt -> v2.txt");
    let worktree = repo.spawn("a");
    for target in ["leaf/data/v1.txt", "leaf/data/v2.txt"] {
        assert!(!is_stamped(&worktree.join(target)), "{target}");
    }
}

#[test]
fn a_retargeted_symlink_is_not_hidden_from_cargo() {
    let repo = TestRepo::new();
    retarget_current_txt(&repo);
    repo.build("a").assert_rebuilt(&["app", "leaf", "mid"]);
    assert_eq!(repo.app("a").marker, "TWO");
}

#[test]
fn a_retargeted_symlink_is_not_hidden_from_cargo_without_a_sandbox() {
    let repo = TestRepo::with_env(&[("FERRY_SANDBOX", "off")]);
    retarget_current_txt(&repo);
    assert_eq!(repo.app("a").marker, "TWO");
}

#[test]
fn a_file_replaced_by_a_symlink_is_not_hidden_from_cargo() {
    let repo = TestRepo::new();
    put(
        &repo,
        &[("data/current.txt", "ONE"), ("data/v2.txt", "TWO")],
    );
    set_leaf(&repo, INCLUDE_CURRENT);
    repo.commit_all("current.txt is a file");
    repo.base();
    link(&repo, "data/current.txt", "v2.txt");
    repo.commit_all("current.txt -> v2.txt");
    let worktree = repo.spawn("a");

    assert!(!is_stamped(&worktree.join("leaf/data/v2.txt")));
    assert_eq!(repo.app("a").marker, "TWO");
}

#[test]
fn a_symlink_replaced_by_a_file_is_not_hidden_from_cargo() {
    let repo = TestRepo::new();
    put(&repo, &[("data/v1.txt", "ONE")]);
    link(&repo, "data/current.txt", "v1.txt");
    set_leaf(&repo, INCLUDE_CURRENT);
    repo.commit_all("current.txt -> v1.txt");
    repo.base();
    put(&repo, &[("data/current.txt", "TWO")]);
    repo.commit_all("current.txt is a file");
    let worktree = repo.spawn("a");

    // What Cargo checks is the input the base read, v1.txt.
    assert!(!is_stamped(&worktree.join("leaf/data/v1.txt")));
    assert_eq!(repo.app("a").marker, "TWO");
}

#[test]
fn a_retargeted_directory_symlink_is_not_hidden_from_cargo() {
    let repo = TestRepo::new();
    put(
        &repo,
        &[
            ("impl_v1/marker.rs", "pub const MARKER: &str = \"ONE\";\n"),
            ("impl_v2/marker.rs", "pub const MARKER: &str = \"TWO\";\n"),
        ],
    );
    link(&repo, "impl", "impl_v1");
    set_leaf(&repo, PATH_MODULE);
    repo.commit_all("impl -> impl_v1");
    repo.base();
    link(&repo, "impl", "impl_v2");
    repo.commit_all("impl -> impl_v2");
    let worktree = repo.spawn("a");

    for reached in [
        "impl_v1",
        "impl_v1/marker.rs",
        "impl_v2",
        "impl_v2/marker.rs",
    ] {
        assert!(
            !is_stamped(&worktree.join("leaf").join(reached)),
            "{reached}"
        );
    }
    assert!(is_stamped(&worktree.join("leaf/src/lib.rs")));
    assert_eq!(repo.app("a").marker, "TWO");
}

#[test]
fn a_path_climbing_out_of_a_retargeted_directory_symlink_is_not_hidden_from_cargo() {
    let repo = TestRepo::new();
    put(
        &repo,
        &[
            ("v1/impl/README", "impl"),
            ("v2/impl/README", "impl"),
            ("v1/VERSION", "ONE"),
            ("v2/VERSION", "TWO"),
        ],
    );
    link(&repo, "impl", "v1/impl");
    // The base reads v1/VERSION; Cargo records it by that canonical path.
    set_leaf(
        &repo,
        "pub fn marker() -> &'static str {\n    include_str!(\"../impl/../VERSION\")\n}\n",
    );
    repo.commit_all("impl -> v1/impl");
    repo.base();
    link(&repo, "impl", "v2/impl");
    repo.commit_all("impl -> v2/impl");
    let worktree = repo.spawn("a");

    assert!(!is_stamped(&worktree.join("leaf/v1/VERSION")));
    assert_eq!(repo.app("a").marker, "TWO");
}

#[test]
fn an_unchanged_symlink_leaves_everything_else_stamped() {
    let repo = TestRepo::new();
    put(
        &repo,
        &[
            ("impl_v1/marker.rs", "pub const MARKER: &str = \"ONE\";\n"),
            ("data/v1.txt", "data"),
            ("data/notes.txt", "notes"),
        ],
    );
    link(&repo, "impl", "impl_v1");
    link(&repo, "data/current.txt", "v1.txt");
    set_leaf(
        &repo,
        &format!(
            "{PATH_MODULE}\npub fn data() -> &'static str {{\n    \
             include_str!(\"../data/current.txt\")\n}}\n"
        ),
    );
    repo.commit_all("symlinks");
    repo.base();
    put(&repo, &[("data/notes.txt", "edited notes")]);
    repo.commit_all("edit notes");

    let report = repo.ferry_json(["spawn", "a"]);
    let worktree = path_of(&report["agent"]["worktree"]);
    // The two symlinks and notes.txt.
    assert_eq!(report["agent"]["stamp"]["skipped"], 3, "{report}");
    for stamped in [
        "leaf/impl_v1",
        "leaf/impl_v1/marker.rs",
        "leaf/data",
        "leaf/data/v1.txt",
    ] {
        assert!(is_stamped(&worktree.join(stamped)), "{stamped}");
    }
    repo.build("a").assert_rebuilt(&[]);
    assert_eq!(repo.app("a").marker, "ONE");
}
