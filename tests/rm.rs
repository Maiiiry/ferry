//! `ferry rm` never destroys work: it refuses dirty worktrees and keeps
//! branches that carry commits.

mod common;

use common::TestRepo;

fn branch_exists(repo: &TestRepo, branch: &str) -> bool {
    let reference = format!("refs/heads/{branch}");
    common::run(
        repo.command("git")
            .args(["show-ref", "--verify", "--quiet", &reference]),
    )
    .status
    .success()
}

#[test]
fn rm_refuses_a_dirty_worktree_without_force() {
    let repo = TestRepo::new();
    repo.base();
    let worktree = repo.spawn("a");

    let untracked = worktree.join("notes.txt");
    repo.write(&untracked, "agent notes\n");
    let refused = repo.ferry(["rm", "a"]).failed();
    assert!(refused.stderr.contains("error:"), "{refused}");
    assert!(refused.stderr.contains("uncommitted"), "{refused}");
    assert!(worktree.is_dir());
    std::fs::remove_file(&untracked).unwrap();

    repo.set_marker(&worktree, "UNSAVED");
    repo.ferry(["rm", "a"]).failed();
    assert!(worktree.is_dir());
    assert_eq!(repo.ferry_json(["list"])["agents"][0]["name"], "a");

    let removed = repo.ferry_json(["rm", "--force", "a"]);
    assert_eq!(removed["branch_deleted"], true, "{removed}");
    assert!(!worktree.exists());
    assert!(!branch_exists(&repo, "ferry/a"));
    assert_eq!(repo.ferry_json(["list"])["agents"], serde_json::json!([]));
}

#[test]
fn rm_keeps_a_branch_with_new_commits() {
    let repo = TestRepo::new();
    repo.base();
    let worktree = repo.spawn("a");
    repo.set_marker(&worktree, "COMMITTED");
    repo.git(&worktree, ["commit", "-q", "-am", "agent work"]);
    let tip = repo.git(&worktree, ["rev-parse", "HEAD"]);

    let removed = repo.ferry_json(["rm", "a"]);
    assert_eq!(removed["branch_deleted"], false, "{removed}");
    assert_eq!(removed["branch_commits"], 1, "{removed}");
    assert!(!worktree.exists());
    assert_eq!(repo.git(repo.root(), ["rev-parse", "ferry/a"]), tip);
}
