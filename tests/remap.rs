//! Experimental remap mode: a warm target forked to another path keeps rustc's
//! incremental state for workspace libraries when builds go through
//! `ferry-rustc`.
//!
//! Reuse is counted the way the research brief did: rustc hard-links
//! unchanged work products into its new session directory, so a reused
//! codegen unit's object file keeps the inode it had in the copied target.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "helpers outside #[test] functions fail the test by panicking"
)]

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use ferry::remap::{TARGET_LINK, WRAPPER_NAME};
use ferry::toolchain::Toolchain;

const LIB: &str = "remaplib";
const BIN: &str = "remapbin";
const MODULES: usize = 12;

#[derive(Debug, Clone, Copy)]
struct Reuse {
    reused: usize,
    total: usize,
}

#[derive(Debug)]
struct Fork {
    lib: Reuse,
    bin: Reuse,
}

#[test]
fn forked_target_keeps_incremental_state_through_the_wrapper() {
    let rustc = Command::new("rustc").arg("-vV").output().unwrap();
    let toolchain = Toolchain::parse(&String::from_utf8_lossy(&rustc.stdout), "");
    if let Err(why) = ferry::remap::supported(&toolchain) {
        eprintln!("skipping the remap test: {why}");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // Canonical, so that macOS's /var -> /private/var does not matter here.
    let root = fs::canonicalize(tmp.path()).unwrap();
    let wrapper = wrapper_in(&root);

    let with = fork_and_edit(&root.join("with"), Some(&wrapper));
    let without = fork_and_edit(&root.join("without"), None);
    for (krate, with, without) in [(LIB, with.lib, without.lib), (BIN, with.bin, without.bin)] {
        eprintln!(
            "{krate}: {}/{} codegen units reused after fork + edit with {WRAPPER_NAME}, {}/{} without",
            with.reused, with.total, without.reused, without.total
        );
    }
    // Only the library is asserted: on targets that embed gdb scripts
    // (Linux), every codegen unit of a binary reads the absolute paths of its
    // dependencies, so the binary rebuilds fully either way.
    let (with, without) = (with.lib, without.lib);
    assert!(with.total > MODULES, "{with:?}");
    assert!(with.reused + 2 >= with.total, "{with:?}");
    assert!(
        without.reused * 10 <= without.total,
        "the control reused codegen units, so the test measures nothing: {without:?}"
    );
}

#[test]
fn wrapper_passes_probes_through_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let wrapper = wrapper_in(tmp.path());
    let direct = Command::new("rustc").arg("-vV").output().unwrap();
    let wrapped = Command::new(&wrapper)
        .args(["rustc", "-vV"])
        .output()
        .unwrap();
    assert!(wrapped.status.success(), "{wrapped:?}");
    assert_eq!(wrapped.stdout, direct.stdout);
    let missing = Command::new(&wrapper)
        .arg(tmp.path().join("no-such-rustc"))
        .arg("-vV")
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(127), "{missing:?}");
}

fn wrapper_in(dir: &Path) -> PathBuf {
    let wrapper = dir.join("bin").join(WRAPPER_NAME);
    fs::create_dir_all(dir.join("bin")).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ferry"), &wrapper).unwrap();
    wrapper
}

/// Builds a workspace at `<dir>/a`, forks it and its target to new paths,
/// deletes the originals, edits both crates and rebuilds in the fork.
fn fork_and_edit(dir: &Path, wrapper: Option<&Path>) -> Fork {
    let (a, b) = (dir.join("a"), dir.join("b"));
    let (target_a, target_b) = (dir.join("target-a"), dir.join("target-b"));
    write_workspace(&a);
    fs::create_dir_all(&target_a).unwrap();
    run(Command::new("git").args(["init", "-q"]).arg(&a));
    ferry::remap::prepare(&a, &target_a).unwrap();
    assert_eq!(cargo_build(&a, wrapper), [BIN, LIB]);

    run(Command::new("cp").arg("-a").arg(&a).arg(&b));
    run(Command::new("cp").arg("-a").arg(&target_a).arg(&target_b));
    fs::remove_dir_all(&a).unwrap();
    fs::remove_dir_all(&target_a).unwrap();
    ferry::remap::prepare(&b, &target_b).unwrap();
    assert_eq!(fs::read_link(b.join(TARGET_LINK)).unwrap(), target_b);

    // Appended, so every existing item keeps its position.
    fs::write(
        b.join(LIB).join("src/lib.rs"),
        modules() + "\npub fn added() -> usize {\n    42\n}\n",
    )
    .unwrap();
    fs::write(
        b.join(BIN).join("src/main.rs"),
        main_rs(&format!(
            "println!(\"{{}} {{}} {{}}\", {LIB}::total(2), total(1), {LIB}::added());"
        )),
    )
    .unwrap();
    let incremental = target_b.join("debug/incremental");
    let before = object_inodes(&incremental);
    assert_eq!(cargo_build(&b, wrapper), [BIN, LIB]);

    let output = Command::new(target_b.join("debug").join(BIN))
        .output()
        .unwrap();
    let total = |x: usize| (0..MODULES).map(|i| x * (i + 1) + i).sum::<usize>();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{} {} 42\n", total(2), total(1))
    );
    let reuse = |krate| {
        let objects = object_inodes(&newest_session(&incremental, krate));
        Reuse {
            reused: objects.intersection(&before).count(),
            total: objects.len(),
        }
    };
    let fork = Fork {
        lib: reuse(LIB),
        bin: reuse(BIN),
    };
    assert_eq!(
        cargo_build(&b, wrapper),
        Vec::<String>::new(),
        "no-op rebuild"
    );
    fs::remove_dir_all(dir).unwrap();
    fork
}

/// A library and a binary using it, each with one module (and so one
/// codegen unit) per `MODULES`.
fn write_workspace(root: &Path) {
    let manifest = |name: &str, deps: &str| {
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n{deps}"
        )
    };
    for name in [LIB, BIN] {
        fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = [\"{LIB}\", \"{BIN}\"]\nresolver = \"3\"\n"),
    )
    .unwrap();
    fs::write(root.join(LIB).join("Cargo.toml"), manifest(LIB, "")).unwrap();
    fs::write(
        root.join(BIN).join("Cargo.toml"),
        manifest(BIN, &format!("{LIB} = {{ path = \"../{LIB}\" }}\n")),
    )
    .unwrap();
    fs::write(root.join(LIB).join("src/lib.rs"), modules()).unwrap();
    fs::write(
        root.join(BIN).join("src/main.rs"),
        main_rs(&format!(
            "println!(\"{{}} {{}}\", {LIB}::total(2), total(1));"
        )),
    )
    .unwrap();
}

fn modules() -> String {
    let mut src = String::new();
    for i in 0..MODULES {
        let factor = i + 1;
        writeln!(
            src,
            "pub mod m{i} {{\n    pub fn value(x: usize) -> usize {{\n        x * {factor} + {i}\n    }}\n}}\n"
        )
        .unwrap();
    }
    let sum: Vec<String> = (0..MODULES).map(|i| format!("m{i}::value(x)")).collect();
    writeln!(
        src,
        "pub fn total(x: usize) -> usize {{\n    {}\n}}",
        sum.join(" + ")
    )
    .unwrap();
    src
}

fn main_rs(body: &str) -> String {
    format!("{}\nfn main() {{\n    {body}\n}}\n", modules())
}

/// Names of the units cargo rebuilt, sorted.
fn cargo_build(dir: &Path, wrapper: Option<&Path>) -> Vec<String> {
    let target = dir.join(TARGET_LINK);
    // Empty values also override wrappers and flags from the user's config.
    let output = Command::new(env!("CARGO"))
        .args(["build", "--message-format=json"])
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_BUILD_BUILD_DIR", &target)
        .env("CARGO_INCREMENTAL", "1")
        .env("RUSTC_WRAPPER", "")
        .env(
            "RUSTC_WORKSPACE_WRAPPER",
            wrapper.map_or(OsStr::new(""), Path::as_os_str),
        )
        .env("RUSTFLAGS", "")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_BOOTSTRAP")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo build in {} failed:\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut rebuilt: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["fresh"] == false)
        .filter_map(|m| m["target"]["name"].as_str().map(str::to_owned))
        .collect();
    rebuilt.sort();
    rebuilt
}

/// The most recent incremental session directory of `krate`.
fn newest_session(incremental: &Path, krate: &str) -> PathBuf {
    let prefix = format!("{krate}-");
    fs::read_dir(incremental)
        .unwrap()
        .map(Result::unwrap)
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .flat_map(|e| fs::read_dir(e.path()).unwrap().map(Result::unwrap))
        .filter(|s| s.file_type().unwrap().is_dir())
        .max_by_key(|s| s.metadata().unwrap().modified().unwrap())
        .unwrap_or_else(|| panic!("no incremental session for {krate}"))
        .path()
}

/// Inodes of every object file under `dir`.
fn object_inodes(dir: &Path) -> HashSet<u64> {
    let mut inodes = HashSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().map(Result::unwrap) {
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                stack.push(entry.path());
            } else if entry.path().extension() == Some(OsStr::new("o")) {
                inodes.insert(meta.ino());
            }
        }
    }
    inodes
}

fn run(command: &mut Command) {
    let status = command.status().unwrap();
    assert!(status.success(), "{command:?} failed with {status}");
}

/// The wrapper needs `RUSTC_BOOTSTRAP` for its `-Z` flag; that must not let
/// a crate use `#![feature]` on a toolchain that would refuse it.
#[test]
fn wrapper_keeps_feature_gates_on_stable() {
    let rustc = Command::new("rustc").arg("-vV").output().unwrap();
    let toolchain = Toolchain::parse(&String::from_utf8_lossy(&rustc.stdout), "");
    if toolchain.is_nightly() || ferry::remap::supported(&toolchain).is_err() {
        eprintln!("skipping the feature-gate test: it needs a stable or beta rustc 1.98+");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let wrapper = wrapper_in(&root);
    let krate = root.join("gated");
    fs::create_dir_all(krate.join("src")).unwrap();
    fs::write(
        krate.join("Cargo.toml"),
        "[package]\nname = \"gated\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        krate.join("src/lib.rs"),
        "#![feature(never_type)]\npub fn none() -> Option<!> { None }\n",
    )
    .unwrap();
    let build = |nightly: bool| {
        let mut cargo = Command::new("cargo");
        cargo
            .args(["build", "--offline"])
            .current_dir(&krate)
            .env("CARGO_TARGET_DIR", root.join("target"))
            .env("RUSTC_WORKSPACE_WRAPPER", &wrapper)
            .env_remove("RUSTC_BOOTSTRAP")
            .env_remove("FERRY_REMAP_NIGHTLY");
        if nightly {
            cargo.env("FERRY_REMAP_NIGHTLY", "1");
        }
        cargo.output().unwrap()
    };

    let refused = build(false);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{stderr}");
    assert!(
        stderr.contains("not in the list of allowed features"),
        "{stderr}"
    );
    let nightly = build(true);
    assert!(
        nightly.status.success(),
        "{}",
        String::from_utf8_lossy(&nightly.stderr)
    );
}
