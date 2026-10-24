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
        let mut child = repo
            .ferry_cmd(["--json", "base", "--watch", "--interval", "0.2"])
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
