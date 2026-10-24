//! Linux namespace sandbox, end to end.
//!
//! `sandbox::enter` is irreversible and needs a single-threaded process,
//! while libtest runs every test on a spawned thread. So this file has its
//! own `main`: each scenario re-executes this binary with `FERRY_TEST_ROLE`
//! set, the child enters the sandbox on its main thread, reports how, and
//! then answers file commands read from stdin, one line each.

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod linux {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio};
    use std::time::{Duration, Instant};
    use std::{env, fs, thread};

    use ferry::fork::TargetSpec;
    use ferry::lock::FileLock;
    use ferry::platform::Capabilities;
    use ferry::sandbox::{self, Entered, Isolation, Plan};

    const ROLE: &str = "FERRY_TEST_ROLE";
    const ROOT: &str = "FERRY_TEST_ROOT";

    #[derive(Clone, Copy)]
    enum Needs {
        Nothing,
        Userns,
        Overlay,
    }

    const TESTS: &[(&str, Needs, fn())] = &[
        ("bind_only_create", Needs::Userns, bind_only_create),
        (
            "overlay_create_leaves_lower_intact",
            Needs::Overlay,
            overlay_create_leaves_lower_intact,
        ),
        (
            "sessions_share_one_overlay",
            Needs::Overlay,
            sessions_share_one_overlay,
        ),
        (
            "member_without_lock_is_joined",
            Needs::Overlay,
            member_without_lock_is_joined,
        ),
        (
            "stale_work_dir_is_cleared",
            Needs::Overlay,
            stale_work_dir_is_cleared,
        ),
        (
            "nested_mount_is_never_mounted_twice",
            Needs::Overlay,
            nested_mount_is_never_mounted_twice,
        ),
        (
            "released_lock_is_waited_for",
            Needs::Overlay,
            released_lock_is_waited_for,
        ),
        (
            "overlapping_sessions_are_not_refused",
            Needs::Overlay,
            overlapping_sessions_are_not_refused,
        ),
        (
            "held_lock_without_member_is_refused",
            Needs::Nothing,
            held_lock_without_member_is_refused,
        ),
        (
            "threaded_process_is_refused",
            Needs::Nothing,
            threaded_process_is_refused,
        ),
        (
            "detect_probes_in_a_child",
            Needs::Nothing,
            detect_probes_in_a_child,
        ),
    ];

    pub fn main() -> ExitCode {
        let args: Vec<String> = env::args().skip(1).collect();
        // `Capabilities::detect` re-executes the current binary as
        // `__probe-sandbox <dir>`; answering it here runs that path for real.
        if let [command, dir] = args.as_slice()
            && command == "__probe-sandbox"
        {
            let result = sandbox::linux::probe_in_child(Path::new(dir));
            println!("{}", serde_json::to_string(&result).unwrap());
            return ExitCode::SUCCESS;
        }
        if let Ok(role) = env::var(ROLE) {
            child(&role, Path::new(&env::var(ROOT).unwrap()));
            return ExitCode::SUCCESS;
        }
        run(&args)
    }

    fn run(args: &[String]) -> ExitCode {
        let filters: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        let selected: Vec<_> = TESTS
            .iter()
            .filter(|(name, ..)| filters.is_empty() || filters.iter().any(|f| name.contains(f)))
            .collect();
        let probe_dir = tempfile::tempdir().unwrap();
        let caps = Capabilities::detect(probe_dir.path());
        println!("\nrunning {} tests", selected.len());
        let (mut passed, mut failed, mut skipped) = (0, Vec::new(), Vec::new());
        for &&(name, needs, test) in &selected {
            let probes = match needs {
                Needs::Nothing => vec![],
                Needs::Userns => vec![("userns", &caps.userns)],
                Needs::Overlay => vec![("userns", &caps.userns), ("overlay", &caps.overlay)],
            };
            let missing: Vec<_> = probes.into_iter().filter(|(_, p)| !p.is_ok()).collect();
            if let Some(&(key, probe)) = missing.iter().find(|(key, _)| required(key)) {
                eprintln!(
                    "{name}: {key} is required by FERRY_TEST_REQUIRE but unavailable: {probe:?}"
                );
                println!("test {name} ... FAILED");
                failed.push(name);
                continue;
            }
            if let Some((key, probe)) = missing.first() {
                eprintln!("skipping {name}: {key} unavailable here: {probe:?}");
                println!("test {name} ... skipped");
                skipped.push(name);
                continue;
            }
            if catch_unwind(AssertUnwindSafe(test)).is_ok() {
                println!("test {name} ... ok");
                passed += 1;
            } else {
                println!("test {name} ... FAILED");
                failed.push(name);
            }
        }
        if !skipped.is_empty() {
            println!("\nskipped: {}", skipped.join(", "));
        }
        println!(
            "\ntest result: {}. {passed} passed; {} failed; {} skipped\n",
            if failed.is_empty() { "ok" } else { "FAILED" },
            failed.len(),
            skipped.len()
        );
        if failed.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }

    /// Whether `FERRY_TEST_REQUIRE` (`userns`, `overlay`, `reflink`) lists
    /// `key`: CI sets it where the capability exists, so a regression in
    /// detection fails the run instead of skipping tests.
    fn required(key: &str) -> bool {
        env::var("FERRY_TEST_REQUIRE").is_ok_and(|v| v.split(',').any(|k| k.trim() == key))
    }

    // --- child side ---

    fn plan(root: &Path, overlay: bool) -> Plan {
        Plan {
            isolation: Isolation::Namespace,
            src: root.join("src"),
            target: if overlay {
                TargetSpec::Overlay {
                    lower: root.join("lower"),
                    upper: root.join("upper"),
                    work: root.join("work"),
                }
            } else {
                TargetSpec::Dir(root.join("target"))
            },
            canonical_src: root.join("mnt/src"),
            canonical_target: root.join("mnt/target"),
            ns_record: overlay.then(|| root.join("agent/ns.json")),
            remap_wrapper: None,
        }
    }

    fn child(role: &str, root: &Path) {
        let entered = match role {
            "dir" => sandbox::enter(&plan(root, false)),
            "overlay" => {
                // Held while entering, as `ferry run` holds the agent lock.
                let agent = fs::File::create(root.join("agent/lock")).unwrap();
                rustix::fs::flock(&agent, rustix::fs::FlockOperation::LockExclusive).unwrap();
                sandbox::enter(&plan(root, true))
            }
            "threaded" => {
                let (keep_alive, wait) = std::sync::mpsc::channel::<()>();
                let thread = thread::spawn(move || wait.recv());
                let entered = sandbox::enter(&plan(root, false));
                drop(keep_alive);
                let _ = thread.join();
                entered
            }
            other => panic!("unknown role {other}"),
        };
        println!(
            "{}",
            match entered {
                Ok(Entered::Created) => "created".to_owned(),
                Ok(Entered::Joined { pid }) => format!("joined {pid}"),
                Ok(Entered::Host) => "host".to_owned(),
                Err(e) => format!("error {e:?}"),
            }
        );
        println!("{}", fs::metadata("/proc/self/ns/mnt").unwrap().ino());
        for line in std::io::stdin().lines() {
            let line = line.unwrap();
            let mut words = line.splitn(3, ' ');
            let command = words.next().unwrap();
            let path = root.join(words.next().unwrap_or_default());
            let reply = match command {
                "read" => fs::read_to_string(&path).map(|s| format!("data {s}")),
                "write" => fs::write(&path, words.next().unwrap_or("")).map(|()| "ok".into()),
                "fill" => {
                    let len = words.next().unwrap().parse().unwrap();
                    fs::write(&path, vec![b'x'; len]).map(|()| "ok".into())
                }
                "rmtree" => fs::remove_dir_all(&path).map(|()| "ok".into()),
                "unlock" => unlock(&path).map(|()| "ok".into()),
                "nest" => nest().map(|()| "ok".into()),
                other => panic!("unknown command {other}"),
            };
            println!("{}", reply.unwrap_or_else(|e| format!("err {e}")));
        }
    }

    /// Releases this process's lock on `lock` without closing the
    /// descriptor, like a daemon that closed the descriptors it inherited.
    fn unlock(lock: &Path) -> std::io::Result<()> {
        for entry in fs::read_dir("/proc/self/fd")? {
            let entry = entry?;
            if fs::read_link(entry.path()).is_ok_and(|target| target == lock) {
                let fd = entry.file_name().to_str().unwrap().parse().unwrap();
                // SAFETY: `fd` is listed in /proc/self/fd, and nothing in this
                // single-threaded process closes it while it is borrowed.
                let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
                rustix::fs::flock(fd, rustix::fs::FlockOperation::Unlock)?;
            }
        }
        Ok(())
    }

    /// Moves this process into a user and mount namespace nested in its
    /// sandbox, like `unshare --user --map-current-user --mount`: it keeps
    /// the overlay mounted but is no longer in the recorded namespace.
    fn nest() -> std::io::Result<()> {
        let uid = rustix::process::getuid().as_raw();
        let gid = rustix::process::getgid().as_raw();
        let flags = rustix::thread::UnshareFlags::NEWUSER | rustix::thread::UnshareFlags::NEWNS;
        // SAFETY: the child runs on its main thread only, and without
        // CLONE_FILES no file descriptor table is split.
        unsafe { rustix::thread::unshare_unsafe(flags) }?;
        fs::write("/proc/self/setgroups", "deny")?;
        fs::write("/proc/self/uid_map", format!("{uid} {uid} 1\n"))?;
        fs::write("/proc/self/gid_map", format!("{gid} {gid} 1\n"))
    }

    // --- parent side ---

    struct Session {
        child: Child,
        stdin: Option<ChildStdin>,
        stdout: BufReader<ChildStdout>,
        /// First line: how the child entered.
        entered: String,
        /// Inode of the child's mount namespace.
        mnt: u64,
    }

    impl Session {
        fn start(root: &Path, role: &str) -> Self {
            let mut session = Self::spawn(root, role);
            session.wait_entered();
            session
        }

        /// Starts the child without waiting for it to enter the sandbox.
        fn spawn(root: &Path, role: &str) -> Self {
            let mut child = Command::new(env::current_exe().unwrap())
                .env(ROLE, role)
                .env(ROOT, root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let stdin = child.stdin.take();
            let stdout = BufReader::new(child.stdout.take().unwrap());
            Self {
                child,
                stdin,
                stdout,
                entered: String::new(),
                mnt: 0,
            }
        }

        fn wait_entered(&mut self) {
            self.entered = self.line();
            self.mnt = self.line().parse().unwrap();
        }

        fn pid(&self) -> u32 {
            self.child.id()
        }

        fn line(&mut self) -> String {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            line.trim_end().to_owned()
        }

        fn send(&mut self, command: &str) -> String {
            writeln!(self.stdin.as_mut().unwrap(), "{command}").unwrap();
            self.line()
        }

        /// Closes stdin and waits for the child to exit.
        fn finish(mut self) {
            drop(self.stdin.take());
            assert!(self.child.wait().unwrap().success());
        }
    }

    /// A store-like directory tree; removed with the forced removal Ferry
    /// uses, because overlayfs leaves a mode-000 `work/work` behind.
    struct Scenario(tempfile::TempDir);

    impl Scenario {
        fn new() -> Self {
            Self::in_dir(tempfile::tempdir().unwrap())
        }

        /// Under the target dir rather than on a tmpfs: unmounting an
        /// overlay syncs its upper dir's filesystem, which can take seconds.
        fn on_disk() -> Self {
            Self::in_dir(tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap())
        }

        fn in_dir(dir: tempfile::TempDir) -> Self {
            for sub in [
                "src",
                "target",
                "lower/sub",
                "upper",
                "work",
                "mnt/src",
                "mnt/target",
                "agent",
            ] {
                fs::create_dir_all(dir.path().join(sub)).unwrap();
            }
            fs::write(dir.path().join("src/hello"), "from the worktree").unwrap();
            fs::write(dir.path().join("lower/base"), "from the base").unwrap();
            fs::write(dir.path().join("lower/sub/f"), "nested").unwrap();
            Self(dir)
        }

        fn root(&self) -> &Path {
            self.0.path()
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.root().join(rel)
        }

        fn users(&self) -> Vec<u32> {
            sandbox::users(&self.path("agent/ns.json")).unwrap()
        }
    }

    impl Drop for Scenario {
        fn drop(&mut self) {
            let _ = ferry::fsutil::remove_dir_all_force(self.root());
        }
    }

    /// Every entry under `dir` with its contents (`None` for directories).
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(path) = stack.pop() {
            for entry in fs::read_dir(&path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path.clone());
                    out.push((path, None));
                } else {
                    let bytes = fs::read(&path).unwrap();
                    out.push((path, Some(bytes)));
                }
            }
        }
        out.sort();
        out
    }

    fn bind_only_create() {
        let tree = Scenario::new();
        let mut a = Session::start(tree.root(), "dir");
        assert_eq!(a.entered, "created");
        assert_eq!(a.send("read mnt/src/hello"), "data from the worktree");
        assert_eq!(a.send("write mnt/src/new edited"), "ok");
        assert_eq!(a.send("write mnt/target/out built"), "ok");
        a.finish();
        assert_eq!(fs::read_to_string(tree.path("src/new")).unwrap(), "edited");
        assert_eq!(
            fs::read_to_string(tree.path("target/out")).unwrap(),
            "built"
        );
        assert_eq!(fs::read_dir(tree.path("mnt/src")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(tree.path("mnt/target")).unwrap().count(), 0);
    }

    fn overlay_create_leaves_lower_intact() {
        let tree = Scenario::new();
        let lower = snapshot(&tree.path("lower"));
        let mut a = Session::start(tree.root(), "overlay");
        assert_eq!(a.entered, "created");
        assert_eq!(a.send("read mnt/target/base"), "data from the base");
        assert_eq!(a.send("write mnt/target/base rebuilt"), "ok");
        assert_eq!(a.send("write mnt/target/new fresh"), "ok");
        assert_eq!(a.send("rmtree mnt/target/sub"), "ok");
        assert_eq!(a.send("read mnt/target/base"), "data rebuilt");
        a.finish();
        assert_eq!(snapshot(&tree.path("lower")), lower);
        assert_eq!(
            fs::read_to_string(tree.path("upper/base")).unwrap(),
            "rebuilt"
        );
        assert_eq!(fs::read_to_string(tree.path("upper/new")).unwrap(), "fresh");
        let whiteout = fs::symlink_metadata(tree.path("upper/sub")).unwrap();
        assert!(whiteout.file_type().is_char_device());
        assert!(tree.path("agent/ns.json").is_file());
    }

    fn sessions_share_one_overlay() {
        let tree = Scenario::new();
        let mut a = Session::start(tree.root(), "overlay");
        assert_eq!(a.entered, "created");
        assert_eq!(a.send("write mnt/target/a from-a"), "ok");
        assert_eq!(tree.users(), vec![a.pid()]);

        let mut b = Session::start(tree.root(), "overlay");
        assert_eq!(b.entered, format!("joined {}", a.pid()));
        assert_eq!(b.mnt, a.mnt);
        assert_eq!(b.send("read mnt/target/a"), "data from-a");
        assert_eq!(b.send("write mnt/target/b from-b"), "ok");
        assert_eq!(a.send("read mnt/target/b"), "data from-b");
        let mut both = vec![a.pid(), b.pid()];
        both.sort_unstable();
        assert_eq!(tree.users(), both);

        // The creator leaving does not end the sandbox: the next session
        // joins the remaining member instead of mounting a second overlay.
        let (a_mnt, b_pid) = (a.mnt, b.pid());
        a.finish();
        assert_eq!(tree.users(), vec![b_pid]);
        let mut c = Session::start(tree.root(), "overlay");
        assert_eq!(c.entered, format!("joined {b_pid}"));
        assert_eq!(c.mnt, a_mnt);
        assert_eq!(c.send("read mnt/target/a"), "data from-a");
        b.finish();
        c.finish();
        assert_eq!(tree.users(), Vec::<u32>::new());

        let mut d = Session::start(tree.root(), "overlay");
        assert_eq!(d.entered, "created");
        assert_eq!(d.send("read mnt/target/a"), "data from-a");
        assert_eq!(d.send("read mnt/target/b"), "data from-b");
        d.finish();
    }

    fn member_without_lock_is_joined() {
        let tree = Scenario::new();
        let mut a = Session::start(tree.root(), "overlay");
        assert_eq!(a.entered, "created");
        assert_eq!(a.send("unlock agent/mount.lock"), "ok");
        assert_eq!(tree.users(), vec![a.pid()]);
        let b = Session::start(tree.root(), "overlay");
        assert_eq!(b.entered, format!("joined {}", a.pid()));
        assert_eq!(b.mnt, a.mnt);
        b.finish();
        a.finish();
    }

    fn stale_work_dir_is_cleared() {
        let tree = Scenario::new();
        let leftover = tree.path("work/work");
        fs::create_dir_all(leftover.join("dir")).unwrap();
        fs::create_dir_all(leftover.join("incompat/volatile")).unwrap();
        fs::write(leftover.join("dir/file"), "stale").unwrap();
        fs::write(tree.path("work/stray"), "stale").unwrap();
        fs::set_permissions(&leftover, fs::Permissions::from_mode(0o000)).unwrap();
        let mut a = Session::start(tree.root(), "overlay");
        assert_eq!(a.entered, "created");
        assert_eq!(a.send("write mnt/target/new written"), "ok");
        a.finish();
        assert_eq!(
            fs::read_to_string(tree.path("upper/new")).unwrap(),
            "written"
        );
        assert!(!tree.path("work/stray").exists());
    }

    /// A process in a nested namespace that dropped its lock keeps the
    /// overlay alive after the recorded namespace is gone: no second overlay
    /// may be mounted on the same upper dir while it lives.
    fn nested_mount_is_never_mounted_twice() {
        let tree = Scenario::new();
        let mut a = Session::start(tree.root(), "overlay");
        assert_eq!(a.entered, "created");
        assert_eq!(a.send("write mnt/target/f from-a"), "ok");
        assert_eq!(a.send("unlock agent/mount.lock"), "ok");
        assert_eq!(a.send("nest"), "ok");
        let record = fs::read(tree.path("agent/ns.json")).unwrap();

        let b = Session::start(tree.root(), "overlay");
        assert_eq!(
            b.entered,
            format!("error OrphanedOverlay {{ pids: \"{}\" }}", a.pid())
        );
        b.finish();
        assert_eq!(fs::read(tree.path("agent/ns.json")).unwrap(), record);
        assert!(tree.path("work/work").exists(), "work dir cleared");
        assert_eq!(tree.users(), vec![a.pid()]);
        assert_eq!(a.send("write mnt/target/f still-a"), "ok");

        a.finish();
        let mut c = Session::start(tree.root(), "overlay");
        assert_eq!(c.entered, "created");
        assert_eq!(c.send("read mnt/target/f"), "data still-a");
        c.finish();
    }

    /// A held lock with no member to join is how an exiting session looks
    /// while the kernel unmounts its overlay.
    fn released_lock_is_waited_for() {
        let tree = Scenario::new();
        let held = FileLock::shared(&tree.path("agent/mount.lock"), "test").unwrap();
        let mut a = Session::spawn(tree.root(), "overlay");
        thread::sleep(Duration::from_millis(500));
        drop(held);
        a.wait_entered();
        assert_eq!(a.entered, "created");
        a.finish();
    }

    /// Sessions that start while earlier ones exit, as in parallel
    /// `ferry run`s: every one enters, and sessions alive at the same time
    /// share one namespace, so one overlay.
    fn overlapping_sessions_are_not_refused() {
        let tree = Scenario::on_disk();
        let sessions: Vec<_> = (0..60_u64)
            .map(|i| {
                let root = tree.root().to_path_buf();
                let session = thread::spawn(move || {
                    let mut s = Session::start(&root, "overlay");
                    let start = Instant::now();
                    if !s.entered.starts_with("error") {
                        // Data to write back while the overlay is unmounted.
                        assert_eq!(s.send(&format!("fill mnt/target/{i} 1000000")), "ok");
                        thread::sleep(Duration::from_millis(i * 37 % 150));
                    }
                    let end = Instant::now();
                    let (entered, mnt) = (s.entered.clone(), s.mnt);
                    s.finish();
                    (entered, mnt, start, end)
                });
                thread::sleep(Duration::from_millis(40));
                session
            })
            .collect();
        let sessions: Vec<_> = sessions.into_iter().map(|s| s.join().unwrap()).collect();
        let refused: Vec<_> = sessions
            .iter()
            .map(|(entered, ..)| entered)
            .filter(|entered| entered.starts_with("error"))
            .collect();
        assert!(
            refused.is_empty(),
            "{} of 60 refused: {refused:?}",
            refused.len()
        );
        for (i, (_, mnt, start, end)) in sessions.iter().enumerate() {
            for (_, other_mnt, other_start, other_end) in &sessions[i + 1..] {
                if start < other_end && other_start < end {
                    assert_eq!(mnt, other_mnt, "two namespaces alive at once");
                }
            }
        }
    }

    fn held_lock_without_member_is_refused() {
        let tree = Scenario::new();
        let _held = FileLock::shared(&tree.path("agent/mount.lock"), "test").unwrap();
        let a = Session::start(tree.root(), "overlay");
        assert_eq!(
            a.entered,
            format!(
                "error LockedOverlay {{ holders: \"pids {}\" }}",
                std::process::id()
            )
        );
        a.finish();
        assert_eq!(fs::read_dir(tree.path("upper")).unwrap().count(), 0);
        assert!(!tree.path("agent/ns.json").exists());
    }

    fn threaded_process_is_refused() {
        let tree = Scenario::new();
        let a = Session::start(tree.root(), "threaded");
        assert_eq!(a.entered, "error Threaded");
        a.finish();
    }

    /// Runs on every host: skipping it where the probe fails would hide
    /// exactly the regressions it is for.
    fn detect_probes_in_a_child() {
        let dir = tempfile::tempdir().unwrap();
        let caps = Capabilities::detect(dir.path());
        for (key, probe) in [("userns", &caps.userns), ("overlay", &caps.overlay)] {
            assert!(
                !probe.is_inconclusive(),
                "{key} probe did not answer: {probe:?}"
            );
            assert!(
                probe.is_ok() || !required(key),
                "{key} is required by FERRY_TEST_REQUIRE but its probe failed: {probe:?}"
            );
        }
        assert_ne!(caps.kernel, "");
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            0,
            "scratch left behind"
        );
    }
}
