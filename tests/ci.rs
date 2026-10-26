//! Cargo 1.99 turns incremental compilation off when `CI` or `TF_BUILD` is
//! set, ahead of every profile, and the switch enters every unit's hash.
//! Ferry's builds set `CARGO_INCREMENTAL=1` then, unless the user chose a
//! setting of their own, and leave both variables as they are.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "helpers outside #[test] functions fail the test by panicking"
)]

mod common;

use std::fs;
use std::path::Path;

use common::{Build, TestRepo, str_of};

/// Requires that Cargo's `-v` log shows rustc compiling `krate`, with
/// incremental compilation if `incremental`.
#[track_caller]
fn assert_incremental(build: &Build, krate: &str, incremental: bool) {
    let rustc = build
        .log
        .lines()
        .find(|l| l.contains(&format!("--crate-name {krate} ")))
        .unwrap_or_else(|| panic!("no rustc call for {krate}:\n{}", build.log));
    assert_eq!(rustc.contains("-C incremental="), incremental, "{rustc}");
}

#[test]
fn agent_builds_with_ci_set_reuse_a_base_warmed_without_it() {
    let repo = TestRepo::new();
    repo.base();
    let worktree = repo.spawn("a");

    Build::run(repo.agent_cargo("a").env("CI", "true").arg("build")).assert_rebuilt(&[]);
    Build::run(
        repo.shimmed_cargo(&worktree)
            .env("CI", "true")
            .args(["test", "--no-run"]),
    )
    .assert_rebuilt(&[]);

    repo.set_marker(&worktree, "EDITED");
    let edited = Build::run(repo.agent_cargo("a").env("CI", "true").arg("build"));
    edited.assert_rebuilt(&["leaf", "mid", "app"]);
    assert_incremental(&edited, "leaf", true);
}

#[test]
fn bases_warmed_with_tf_build_set_are_incremental() {
    let repo = TestRepo::new();
    let report = common::run(repo.ferry_cmd(["--json", "base"]).env("TF_BUILD", "True"))
        .ok()
        .json();
    let incremental = repo
        .base_dir(str_of(&report["base"]["id"]))
        .join("target/debug/incremental");
    let sessions: Vec<String> = fs::read_dir(&incremental)
        .into_iter()
        .flatten()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        sessions.iter().any(|s| s.starts_with("leaf-")),
        "{}: {sessions:?}",
        incremental.display()
    );

    repo.spawn("a");
    repo.build("a").assert_rebuilt(&[]);
}

/// Under `CI`, Ferry's `CARGO_INCREMENTAL=1` makes release units
/// incremental, so they differ from those of a base warmed without `CI`
/// (dev units do not). A refresh across that difference must not keep the
/// seed's release units: a plain agent's release build would find them
/// fresh, holding the seed's code.
#[test]
fn a_release_build_after_a_refresh_with_ci_set_gets_the_new_code() {
    let repo = TestRepo::with_default_config(&[]);
    repo.write(
        &repo.root().join("ferry.toml"),
        "[base]\ntargets = [\"build\"]\nprofiles = [\"dev\", \"release\"]\n",
    );
    repo.commit_all("A: warm dev and release");
    let a = repo.base();

    repo.set_marker(repo.root(), "LEAF_B");
    repo.commit_all("B: source");
    let b = common::run(repo.ferry_cmd(["--json", "base"]).env("CI", "true"))
        .ok()
        .json();
    assert_eq!(b["base"]["seeded_from"], a.as_str(), "{b}");

    repo.spawn("agent");
    repo.build("agent").assert_rebuilt(&[]);
    let release = ["run", "-q", "--release", "-p", "app"];
    let run = common::run(repo.agent_cargo("agent").args(release)).ok();
    assert!(run.stdout.lines().any(|l| l == "marker=LEAF_B"), "{run}");
}

/// A nested workspace has a target dir of its own (`ferry-ws`), and its
/// builds get `CARGO_INCREMENTAL=1` like the checkout's workspace, both in
/// the warm commands and in agents: agents with and without `CI` reuse what
/// a base warmed with it built.
#[test]
fn nested_workspaces_reuse_a_base_warmed_with_ci_set() {
    let repo = TestRepo::with_env(&[(
        "FERRY_BASE_COMMAND",
        "cargo build --workspace && cargo build --manifest-path fuzz/Cargo.toml",
    )]);
    let fuzz = repo.root().join("fuzz");
    fs::create_dir_all(fuzz.join("fz/src")).unwrap();
    fs::write(
        fuzz.join("Cargo.toml"),
        "[workspace]\nmembers = [\"fz\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        fuzz.join("fz/Cargo.toml"),
        "[package]\nname = \"fz\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(fuzz.join("fz/src/main.rs"), "fn main() {}\n").unwrap();
    common::run(
        repo.cargo_in(&fuzz)
            .args(["generate-lockfile", "--offline"]),
    )
    .ok();
    repo.commit_all("nested workspace");
    common::run(repo.ferry_cmd(["base"]).env("CI", "true")).ok();
    repo.spawn("a");

    let nested = ["build", "--manifest-path", "fuzz/Cargo.toml"];
    Build::run(repo.agent_cargo("a").env("CI", "true").args(nested)).assert_rebuilt(&[]);
    Build::run(repo.agent_cargo("a").args(nested)).assert_rebuilt(&[]);
}

#[test]
fn ci_leaves_cargo_incremental_alone() {
    let repo = TestRepo::new();
    repo.base();
    repo.spawn("a");

    let build = Build::run(
        repo.agent_cargo("a")
            .env("CI", "true")
            .env("CARGO_INCREMENTAL", "0")
            .arg("build"),
    );
    assert_incremental(&build, "leaf", false);
}

#[test]
fn ci_leaves_build_incremental_alone() {
    let repo = TestRepo::new();
    repo.base();
    let worktree = repo.spawn("a");

    let config = worktree.join(".cargo/config.toml");
    fs::create_dir(config.parent().unwrap()).unwrap();
    repo.write(&config, "[build]\nincremental = false\n");
    let build = Build::run(repo.agent_cargo("a").env("CI", "true").arg("build"));
    assert_incremental(&build, "leaf", false);

    fs::remove_file(&config).unwrap();
    repo.set_marker(&worktree, "EDITED");
    let build = Build::run(repo.agent_cargo("a").env("CI", "true").args([
        "build",
        "--config",
        "build.incremental=false",
    ]));
    assert_incremental(&build, "leaf", false);
}

/// A package whose build script, test and binary report the `CI` they see.
fn ci_probe(root: &Path) {
    let files = [
        (
            "Cargo.toml",
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "build.rs",
            "fn main() {\n    println!(\"cargo::rerun-if-env-changed=CI\");\n    \
             let out = std::env::var(\"OUT_DIR\").unwrap();\n    \
             std::fs::write(format!(\"{out}/ci\"), std::env::var(\"CI\").unwrap_or_default()).unwrap();\n}\n",
        ),
        (
            "src/lib.rs",
            "pub const BUILD_SCRIPT_CI: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/ci\"));\n",
        ),
        (
            "src/main.rs",
            "fn main() {\n    print!(\"{}\", std::env::var(\"CI\").unwrap_or_default());\n}\n",
        ),
        (
            "tests/ci.rs",
            "#[test]\nfn sees_ci() {\n    let expected = std::env::var(\"EXPECTED_CI\").unwrap();\n    \
             assert_eq!(probe::BUILD_SCRIPT_CI, expected, \"build script\");\n    \
             assert_eq!(std::env::var(\"CI\").unwrap(), expected, \"test binary\");\n}\n",
        ),
    ];
    for (path, contents) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

#[test]
fn what_cargo_runs_still_sees_ci() {
    const VALUE: &str = "set by an agent tool";
    let repo = TestRepo::with_workspace(&[], ci_probe);
    repo.base();
    repo.spawn("a");

    common::run(
        repo.agent_cargo("a")
            .env("CI", VALUE)
            .env("EXPECTED_CI", VALUE)
            .arg("test"),
    )
    .ok();
    let run = common::run(repo.agent_cargo("a").env("CI", VALUE).args(["run", "-q"])).ok();
    assert_eq!(run.stdout, VALUE);
}
