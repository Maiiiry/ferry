//! Agents working at the same time stay independent.

mod common;

use std::collections::BTreeSet;
use std::thread;

use common::{Build, Snapshot, TestRepo, str_of};

/// Runs `f` for every agent on its own thread, propagating the first panic
/// with its original message.
fn in_parallel<T: Send>(agents: &[&str], f: impl Fn(&str) -> T + Sync) -> Vec<T> {
    thread::scope(|s| {
        let handles: Vec<_> = agents
            .iter()
            .map(|name| {
                let f = &f;
                s.spawn(move || f(name))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

#[test]
fn concurrent_builds_each_see_only_their_own_edit() {
    let repo = TestRepo::new();
    let base = repo.base();
    let before = Snapshot::of(&repo.base_dir(&base));
    let agents = ["a1", "a2", "a3"];
    for name in agents {
        let worktree = repo.spawn(name);
        repo.set_marker(&worktree, &format!("MARKER_{name}"));
    }

    let builds: Vec<Build> = in_parallel(&agents, |name| repo.build(name));
    for build in &builds {
        build.assert_rebuilt(&["leaf", "mid", "app"]);
    }
    for name in agents {
        assert_eq!(repo.app(name).marker, format!("MARKER_{name}"));
    }
    before.assert_unchanged(&Snapshot::of(&repo.base_dir(&base)));
}

#[test]
fn concurrent_spawns_all_succeed() {
    let repo = TestRepo::new();
    repo.base();
    let agents = ["s1", "s2", "s3", "s4"];
    in_parallel(&agents, |name| repo.ferry(["spawn", name]).ok());

    let list = repo.ferry_json(["list"]);
    let listed = list["agents"].as_array().unwrap();
    assert_eq!(listed.len(), agents.len(), "{list}");
    assert!(listed.iter().all(|a| a["state"] == "ready"), "{list}");
    let worktrees: BTreeSet<&str> = listed.iter().map(|a| str_of(&a["worktree"])).collect();
    assert_eq!(worktrees.len(), agents.len(), "{list}");

    for build in in_parallel(&agents, |name| repo.build(name)) {
        build.assert_rebuilt(&[]);
    }
}

/// Reflink sandboxes are plain bind mounts of one host directory: a build
/// next to a live session needs no mount protocol and sees the same target.
#[test]
fn concurrent_sessions_of_a_reflink_agent_share_its_target() {
    let repo = TestRepo::with_env(&[("FERRY_BACKEND", "reflink")]);
    if !repo.require_reflink() {
        return;
    }
    repo.base();
    let worktree = repo.spawn("a");
    let mut holder = repo
        .ferry_cmd(["run", "a", "--", "sleep", "30"])
        .spawn()
        .unwrap();
    thread::sleep(std::time::Duration::from_millis(500));
    repo.set_marker(&worktree, "EDITED");
    repo.build("a").assert_rebuilt(&["leaf", "mid", "app"]);
    repo.build("a").assert_rebuilt(&[]);
    assert_eq!(repo.app("a").marker, "EDITED");
    let _ = holder.kill();
    let _ = holder.wait();
    for file in ["ns.json", "mount.lock"] {
        assert!(!repo.store().join("agents/a").join(file).exists(), "{file}");
    }
}
