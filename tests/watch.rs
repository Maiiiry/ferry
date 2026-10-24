//! `ferry base --watch` builds a base each time the revision it follows
//! settles on a new commit, and leaves the current base alone otherwise.

// clippy's allow-unwrap-in-tests covers only #[test] functions, not their
// helpers.
#![allow(clippy::unwrap_used)]

mod common;

use std::io::BufRead as _;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::TestRepo;
use serde_json::Value;

/// A running `ferry --json base --watch`, killed on drop, with its events
/// and stderr collected as they arrive.
struct Watcher {
    child: Child,
    events: Arc<Mutex<Vec<Value>>>,
    stderr: Arc<Mutex<String>>,
}

impl Watcher {
    fn start(repo: &TestRepo) -> Self {
        Self::start_with(repo, &[])
    }

    /// With `args` after `ferry --json base --watch --interval 0.2`.
    fn start_with(repo: &TestRepo, args: &[&str]) -> Self {
        let mut child = repo
            .ferry_cmd(["--json", "base", "--watch", "--interval", "0.2"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(String::new()));
        let (sink, out) = (Arc::clone(&events), child.stdout.take().unwrap());
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                sink.lock()
                    .unwrap()
                    .push(serde_json::from_str(&line).unwrap());
            }
        });
        let (sink, err) = (Arc::clone(&stderr), child.stderr.take().unwrap());
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                let mut sink = sink.lock().unwrap();
                sink.push_str(&line);
                sink.push('\n');
            }
        });
        Self {
            child,
            events,
            stderr,
        }
    }

    fn events(&self, kind: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["event"] == kind)
            .cloned()
            .collect()
    }

    /// Waits for the `n`th event of `kind` and returns it.
    #[track_caller]
    fn wait_for(&self, kind: &str, n: usize) -> Value {
        let arrived =
            common::eventually_for(Duration::from_secs(120), || self.events(kind).len() >= n);
        assert!(
            arrived,
            "no `{kind}` event #{n}; events: {:?}\nstderr:\n{}",
            self.events.lock().unwrap(),
            self.stderr.lock().unwrap()
        );
        self.events(kind).swap_remove(n - 1)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn current_commit(repo: &TestRepo) -> Value {
    let list = repo.ferry_json(["list"]);
    list["bases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["current"] == true)
        .map_or(Value::Null, |b| b["meta"]["commit"].clone())
}

#[test]
fn the_watcher_builds_a_base_for_each_new_commit() {
    let repo = TestRepo::new();
    let watcher = Watcher::start(&repo);
    let first = watcher.wait_for("built", 1);
    let head = repo.git(repo.root(), ["rev-parse", "HEAD"]);
    assert_eq!(first["commit"], head.as_str(), "{first}");

    repo.set_marker(repo.root(), "LEAF_V2");
    let v2 = repo.commit_all("v2");
    let second = watcher.wait_for("built", 2);
    assert_eq!(second["commit"], v2.as_str(), "{second}");
    assert_eq!(
        second["report"]["base"]["seeded_from"], first["report"]["base"]["id"],
        "{second}"
    );
    assert_eq!(current_commit(&repo), v2.as_str());
    assert_eq!(watcher.events("building").len(), 2);

    repo.spawn("a");
    repo.build("a").assert_rebuilt(&[]);
    assert_eq!(repo.app("a").marker, "LEAF_V2");
}

#[test]
fn a_second_watcher_refuses_to_start() {
    let repo = TestRepo::new();
    let watcher = Watcher::start(&repo);
    watcher.wait_for("built", 1);
    let second = repo.ferry(["base", "--watch"]).failed();
    assert!(second.stderr.contains("already watching"), "{second}");
}

#[test]
fn a_failing_commit_keeps_the_previous_base_and_the_watcher_running() {
    let repo = TestRepo::new();
    let watcher = Watcher::start(&repo);
    let first = watcher.wait_for("built", 1);

    repo.write(&repo.root().join("leaf/src/lib.rs"), "this is not rust\n");
    let broken = repo.commit_all("broken");
    let failed = watcher.wait_for("failed", 1);
    assert_eq!(failed["commit"], broken.as_str(), "{failed}");
    assert_eq!(current_commit(&repo), first["commit"]);

    repo.set_marker(repo.root(), "LEAF_FIXED");
    let fixed = repo.commit_all("fixed");
    let built = watcher.wait_for("built", 2);
    assert_eq!(built["commit"], fixed.as_str(), "{built}");
    assert_eq!(
        watcher.events("failed").len(),
        1,
        "the broken commit was retried"
    );
}

#[test]
fn bisect_does_not_trigger_builds() {
    let repo = TestRepo::new();
    let good = repo.git(repo.root(), ["rev-parse", "HEAD"]);
    repo.set_marker(repo.root(), "LEAF_B");
    repo.commit_all("b");
    repo.set_marker(repo.root(), "LEAF_C");
    let bad = repo.commit_all("c");
    let watcher = Watcher::start(&repo);
    watcher.wait_for("built", 1);

    repo.git(repo.root(), ["bisect", "start", &bad, &good]);
    assert_ne!(repo.git(repo.root(), ["rev-parse", "HEAD"]), bad);
    std::thread::sleep(Duration::from_millis(1500));
    repo.git(repo.root(), ["bisect", "reset"]);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(watcher.events("building").len(), 1);
}

#[test]
fn a_revision_that_does_not_resolve_is_refused_at_startup() {
    let repo = TestRepo::new();
    let mut child = repo
        .ferry_cmd(["base", "--watch", "--rev", "nosuch"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let exited = common::eventually_for(Duration::from_secs(30), || {
        child.try_wait().unwrap().is_some()
    });
    if !exited {
        let _ = child.kill();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        exited,
        "the watcher kept polling a revision that does not exist"
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`nosuch` does not name a commit"),
        "{stderr}"
    );
}

#[test]
fn with_gc_the_watcher_collects_replaced_bases_and_reports_gc_failures() {
    let repo = TestRepo::new();
    repo.ferry(["list"]).ok();
    // An unfinished base gc must collect, whose lock gc cannot take: a
    // directory where the lock file goes. Base builds never lock it.
    let stale = "0000000-00000000";
    let bases = repo.store().join("bases");
    std::fs::create_dir_all(bases.join(stale)).unwrap();
    let stale_lock = bases.join(format!("{stale}.lock"));
    std::fs::create_dir(&stale_lock).unwrap();
    let watcher = Watcher::start_with(&repo, &["--gc"]);
    let first = watcher.wait_for("built", 1);
    let failed = watcher.wait_for("gc-failed", 1);
    assert!(failed["error"].to_string().contains(".lock"), "{failed}");

    std::fs::remove_dir(&stale_lock).unwrap();
    repo.set_marker(repo.root(), "LEAF_V2");
    repo.commit_all("v2");
    watcher.wait_for("built", 2);
    let collected = watcher.wait_for("collected", 1);
    let base1 = first["report"]["base"]["id"].as_str().unwrap();
    let mut removed = collected["report"]["removed"].clone();
    removed
        .as_array_mut()
        .unwrap()
        .sort_by_key(ToString::to_string);
    assert_eq!(removed, serde_json::json!([stale, base1]), "{collected}");
    assert!(!repo.base_dir(base1).exists());
    assert_eq!(watcher.events("gc-failed").len(), 1);
}

/// A rebase that stops at a conflict leaves `HEAD` at a commit of its own.
#[test]
fn a_rebase_in_progress_does_not_trigger_builds() {
    let repo = TestRepo::new();
    let git = |args: &[&str]| repo.git(repo.root(), args);
    git(&["checkout", "-q", "-b", "topic"]);
    repo.set_asset(repo.root(), "a.txt", "topic");
    repo.commit_all("T1: applies cleanly");
    repo.set_marker(repo.root(), "LEAF_TOPIC");
    let topic = repo.commit_all("T2: conflicts");
    git(&["checkout", "-q", "main"]);
    repo.set_marker(repo.root(), "LEAF_MAIN");
    let main = repo.commit_all("M1");
    git(&["checkout", "-q", "topic"]);
    let watcher = Watcher::start(&repo);
    assert_eq!(watcher.wait_for("built", 1)["commit"], topic.as_str());

    common::run(repo.command("git").args(["rebase", "-q", "main"])).failed();
    let stopped_at = git(&["rev-parse", "HEAD"]);
    assert!(stopped_at != topic && stopped_at != main, "{stopped_at}");
    std::thread::sleep(Duration::from_millis(1500));
    git(&["rebase", "--abort"]);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(watcher.events("building").len(), 1);
}

/// `--rev main` moving (a fetch, a push from another worktree) while the
/// watcher's worktree is in a merge waits for the merge to end.
#[test]
fn a_merge_in_progress_defers_builds() {
    let repo = TestRepo::new();
    let git = |args: &[&str]| repo.git(repo.root(), args);
    git(&["checkout", "-q", "-b", "topic"]);
    repo.set_marker(repo.root(), "LEAF_TOPIC");
    repo.commit_all("T1");
    git(&["checkout", "-q", "main"]);
    repo.set_marker(repo.root(), "LEAF_MAIN");
    let main = repo.commit_all("M1");
    git(&["checkout", "-q", "-b", "next"]);
    repo.set_asset(repo.root(), "a.txt", "next");
    let next = repo.commit_all("X");
    git(&["checkout", "-q", "topic"]);
    let watcher = Watcher::start_with(&repo, &["--rev", "main"]);
    assert_eq!(watcher.wait_for("built", 1)["commit"], main.as_str());

    common::run(repo.command("git").args(["merge", "-q", "main"])).failed();
    git(&["branch", "-f", "main", "next"]);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(watcher.events("building").len(), 1);
    git(&["merge", "--abort"]);
    assert_eq!(watcher.wait_for("built", 2)["commit"], next.as_str());
}
