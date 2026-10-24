//! `ferry setup apparmor`: prints the plan, asks, runs the one `sudo`
//! command, and verifies the result as the user. The plan and progress go to
//! stderr, so `--json` keeps stdout to the report.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde::Serialize;

use super::SetupCommand;
use super::output::{self, BOLD, DIM};
use crate::config::shell_quote;
use crate::doctor::probe_detail;
use crate::setup::{self, Decision, Facts, InstallPlan, Outcome, Verification};

#[derive(Debug, Serialize)]
struct Report {
    action: &'static str,
    outcome: Outcome,
    reason: String,
    binary: Option<PathBuf>,
    profile_path: Option<PathBuf>,
    steps: Vec<String>,
    command: Vec<String>,
    verified: Option<Verification>,
    warnings: Vec<String>,
    #[serde(skip)]
    hint: Option<String>,
}

impl Report {
    fn new(action: &'static str, outcome: Outcome, reason: String) -> Self {
        Self {
            action,
            outcome,
            reason,
            binary: None,
            profile_path: None,
            steps: Vec::new(),
            command: Vec::new(),
            verified: None,
            warnings: Vec::new(),
            hint: None,
        }
    }

    fn fail(&mut self, outcome: Outcome, reason: String, hint: Option<&str>) {
        self.outcome = outcome;
        self.reason = reason;
        self.hint = hint.map(Into::into);
    }
}

#[derive(Debug, Clone, Copy)]
struct Opts {
    yes: bool,
    dry_run: bool,
    json: bool,
}

pub(super) fn run(what: &SetupCommand, json: bool) -> anyhow::Result<ExitCode> {
    let SetupCommand::Apparmor {
        yes,
        dry_run,
        uninstall,
        bin_dir,
    } = what;
    let opts = Opts {
        yes: *yes,
        dry_run: *dry_run,
        json,
    };
    let facts = Facts::gather(bin_dir)?;
    let report = if *uninstall {
        uninstall_cmd(&facts, opts)?
    } else {
        install(&facts, opts)?
    };
    if json {
        output::json(&report)?;
    } else {
        print_result(&report);
    }
    Ok(match report.outcome {
        Outcome::Refused | Outcome::Failed => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    })
}

fn install(facts: &Facts, opts: Opts) -> anyhow::Result<Report> {
    let plan = match setup::decide(facts) {
        Decision::Stop { outcome, reason } => return Ok(Report::new("install", outcome, reason)),
        Decision::Run(plan) => plan,
    };
    let argv = plan.sudo_argv(std::process::id(), non_interactive(opts));
    let mut report = Report {
        binary: Some(plan.target.clone()),
        profile_path: Some(plan.profile_path.clone()),
        steps: plan.steps(),
        command: argv.clone(),
        warnings: plan.warnings.clone(),
        ..Report::new("install", Outcome::Planned, why(facts))
    };
    write_install_plan(&mut anstream::stderr().lock(), &report, &plan);
    if opts.dry_run {
        report.hint = Some("dry run: nothing changed".into());
        return Ok(report);
    }
    if let Err((reason, hint)) = consent(opts) {
        report.fail(Outcome::Refused, reason, hint);
        return Ok(report);
    }
    match sudo(&argv, plan.profile.as_bytes()) {
        Err(reason) => {
            report.fail(
                Outcome::Failed,
                reason,
                Some("as root, follow the manual steps in the troubleshooting guide (AppArmor on Ubuntu)"),
            );
            return Ok(report);
        }
        Ok(false) => {
            report.fail(
                Outcome::Failed,
                after_failure(&plan),
                Some("re-run `ferry setup apparmor`, or undo with `ferry setup apparmor --uninstall`"),
            );
            return Ok(report);
        }
        Ok(true) => {}
    }
    verify_install(&mut report, &plan)?;
    Ok(report)
}

/// Checks, as the user, that the profiled binary is this ferry and can
/// create user namespaces, then points the cargo shim at it.
fn verify_install(report: &mut Report, plan: &InstallPlan) -> anyhow::Result<()> {
    let target = &plan.target;
    if !setup::holds_binary(target, Path::new(setup::SELF_EXE)) {
        report.fail(
            Outcome::Failed,
            format!(
                "{} is not a root-owned copy of this ferry ({}) after the install",
                target.display(),
                plan.exe.display()
            ),
            Some("re-run `ferry setup apparmor`"),
        );
        return Ok(());
    }
    let verified = setup::verify(target, &crate::shim::home()?);
    let label = verified.label.clone().unwrap_or_else(|| "none".into());
    if !verified.userns.is_ok() {
        report.fail(
            Outcome::Failed,
            format!(
                "{} still cannot create user namespaces ({}; probe label {label}): {}",
                target.display(),
                probe_detail(&verified.userns),
                setup::label_hint(verified.label.as_deref())
            ),
            Some("nothing was rolled back; undo with `ferry setup apparmor --uninstall`"),
        );
        report.verified = Some(verified);
        return Ok(());
    }
    report.verified = Some(verified);
    // The shim links to whichever ferry last ran it.
    let shim = Command::new(target)
        .arg("shim")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status();
    if !shim.is_ok_and(|s| s.success()) {
        report.warnings.push(format!(
            "could not point the cargo shim at {0}: run `{0} shim`",
            target.display()
        ));
    }
    if let Some(first) = setup::first_on_path("ferry")
        && std::fs::canonicalize(&first).unwrap_or(first.clone()) != *target
    {
        report.warnings.push(format!(
            "the first ferry on PATH is {}, which the profile does not cover: put {} first on \
             PATH or remove that copy, or agents keep building at host paths",
            first.display(),
            target.parent().unwrap_or(target).display()
        ));
    }
    report.outcome = Outcome::Done;
    report.reason = format!(
        "{} can create user namespaces (probe label {label})",
        target.display()
    );
    report.hint = Some(
        "run `ferry base` in each repository; agents spawned earlier keep building at host \
         paths until they are spawned again"
            .into(),
    );
    Ok(())
}

fn uninstall_cmd(facts: &Facts, opts: Opts) -> anyhow::Result<Report> {
    let plan = match setup::decide_uninstall(facts) {
        Decision::Stop { outcome, reason } => {
            return Ok(Report::new("uninstall", outcome, reason));
        }
        Decision::Run(plan) => plan,
    };
    let argv = plan.sudo_argv(non_interactive(opts));
    let mut report = Report {
        binary: plan.binaries.first().cloned(),
        profile_path: plan.files.first().cloned(),
        steps: plan.steps(),
        command: argv.clone(),
        ..Report::new(
            "uninstall",
            Outcome::Planned,
            "Ferry's AppArmor profile is installed".into(),
        )
    };
    let mut err = anstream::stderr().lock();
    let _ = writeln!(err, "One command runs as root (sudo). It will:");
    print_steps(&mut err, &report.steps);
    let _ = writeln!(
        err,
        "\n{DIM}--- root script (sh -euc) ---{DIM:#}\n{}{DIM}--- command ---{DIM:#}\n{}\n",
        setup::UNINSTALL_SCRIPT,
        display_argv(&argv, setup::UNINSTALL_SCRIPT)
    );
    drop(err);
    if opts.dry_run {
        report.hint = Some("dry run: nothing changed".into());
        return Ok(report);
    }
    if let Err((reason, hint)) = consent(opts) {
        report.fail(Outcome::Refused, reason, hint);
        return Ok(report);
    }
    match sudo(&argv, b"") {
        Ok(true) => {}
        Ok(false) => {
            report.fail(
                Outcome::Failed,
                "the root script failed; its `ferry:` lines above show how far it got".into(),
                Some("re-run `ferry setup apparmor --uninstall`"),
            );
            return Ok(report);
        }
        Err(reason) => {
            report.fail(Outcome::Failed, reason, None);
            return Ok(report);
        }
    }
    for binary in plan.binaries.iter().filter(|b| b.exists()) {
        report.warnings.push(format!(
            "{0} is left in place; remove it with `sudo rm {0}` if you installed it only for this",
            binary.display()
        ));
    }
    for local in ["local/ferry", "local/unpriv_ferry"] {
        let path = facts.profile_dir.join(local);
        if path.exists() {
            report.warnings.push(format!(
                "{} is left in place (it is yours; Ferry never writes it)",
                path.display()
            ));
        }
    }
    report.outcome = Outcome::Done;
    report.reason = "removed Ferry's AppArmor profile".into();
    if let Some(binary) = plan.binaries.iter().find(|b| b.exists()) {
        let verified = setup::verify(binary, &crate::shim::home()?);
        report.reason = format!(
            "removed Ferry's AppArmor profile; {} now: {}",
            binary.display(),
            match &verified.userns {
                crate::platform::Probe::Ok => "namespaces still work".into(),
                other => format!("no namespaces ({})", probe_detail(other)),
            }
        );
        report.verified = Some(verified);
    }
    Ok(report)
}

/// `sudo -n` only when nobody can answer a password prompt.
fn non_interactive(opts: Opts) -> bool {
    opts.yes && !std::io::stdin().is_terminal()
}

/// Why the setup is needed: the sysctl, the probe's failure and its label.
fn why(facts: &Facts) -> String {
    format!(
        "kernel.apparmor_restrict_unprivileged_userns = 1; probe: {}{}",
        probe_detail(&facts.caps.userns),
        facts
            .caps
            .probe_label
            .as_deref()
            .map(|l| format!("; label {l}"))
            .unwrap_or_default()
    )
}

/// The user's go-ahead, or why there is none (and a hint).
fn consent(opts: Opts) -> Result<(), (String, Option<&'static str>)> {
    if opts.yes {
        return Ok(());
    }
    let hint = Some("review the plan above and re-run with --yes, or use --dry-run");
    if opts.json {
        return Err((
            "refusing to run sudo without confirmation: --json needs --yes".into(),
            hint,
        ));
    }
    if !std::io::stdin().is_terminal() {
        return Err((
            "refusing to run sudo without confirmation: stdin is not a terminal".into(),
            hint,
        ));
    }
    let mut err = anstream::stderr();
    let _ = write!(err, "Run it? [y/N] ");
    let _ = err.flush();
    if setup::confirm(std::io::stdin().lock()) {
        Ok(())
    } else {
        Err(("cancelled; nothing changed".into(), None))
    }
}

/// Runs `argv` with `input` on its stdin; sudo prompts on the terminal and
/// its errors go straight to stderr.
fn sudo(argv: &[String], input: &[u8]) -> Result<bool, String> {
    let Some(sudo) = setup::first_on_path("sudo") else {
        return Err("sudo is not on PATH, so nothing was run".into());
    };
    let mut child = Command::new(&sudo)
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", sudo.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        // sudo may exit without reading, e.g. when -n meets a password.
        let _ = stdin.write_all(input);
    }
    child
        .wait()
        .map(|s| s.success())
        .map_err(|e| format!("cannot wait for sudo: {e}"))
}

/// What a failed root script may already have changed.
fn after_failure(plan: &InstallPlan) -> String {
    let mut changed = Vec::new();
    if plan.install && setup::holds_binary(&plan.target, Path::new(setup::SELF_EXE)) {
        changed.push(format!("{} is this ferry now", plan.target.display()));
    }
    if std::fs::read_to_string(&plan.profile_path).is_ok_and(|t| t == plan.profile) {
        changed.push(format!(
            "{} holds the new profile",
            plan.profile_path.display()
        ));
    }
    if changed.is_empty() {
        "sudo or the root script failed before changing anything Ferry can see".into()
    } else {
        format!(
            "the root script failed (its `ferry:` lines above show how far it got): {}, but \
             the profile may not be loaded",
            changed.join(" and ")
        )
    }
}

fn write_install_plan(err: &mut impl Write, report: &Report, plan: &InstallPlan) {
    let target = plan.target.display();
    let _ = writeln!(
        err,
        "{BOLD}Ubuntu's AppArmor policy blocks the namespace sandbox here{BOLD:#}\n({}).\n",
        report.reason
    );
    let _ = writeln!(err, "One command runs as root (sudo). It will:");
    print_steps(err, &report.steps);
    let _ = writeln!(
        err,
        "Then, as you: check that {target} can create a user namespace, and point the cargo \
         shim at it.\n\n\
         The profile lets {target} create user namespaces and mount in them; everything it \
         runs (cargo, build scripts, tests, your agent) runs under unpriv_ferry, with no \
         capabilities there. Any local process that can run {target} can use it to create \
         user namespaces (see SECURITY.md).\n\n\
         {DIM}--- {} ---{DIM:#}\n{}\
         {DIM}--- root script (sh -euc) ---{DIM:#}\n{}\
         {DIM}--- command (the profile goes on stdin) ---{DIM:#}\n{}\n\n\
         Undo: ferry setup apparmor --uninstall",
        plan.profile_path.display(),
        plan.profile,
        setup::ROOT_SCRIPT,
        display_argv(&report.command, setup::ROOT_SCRIPT)
    );
}

fn print_steps(err: &mut impl Write, steps: &[String]) {
    for (i, step) in steps.iter().enumerate() {
        let _ = writeln!(err, "  {}. {step}", i + 1);
    }
}

/// `argv` as a shell command line, with the script shown above abbreviated.
fn display_argv(argv: &[String], script: &str) -> String {
    argv.iter()
        .map(|a| {
            if a == script {
                "\"$SCRIPT\"".to_owned()
            } else {
                shell_quote(a)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_result(report: &Report) {
    for warning in &report.warnings {
        output::warning(warning);
    }
    let reason = &report.reason;
    let message = match report.outcome {
        Outcome::Refused | Outcome::Failed => {
            output::error(reason, &[], report.hint.as_deref());
            return;
        }
        Outcome::Planned => None,
        Outcome::NotApplicable => Some(format!("not applicable: {reason}")),
        Outcome::NotNeeded => Some(format!("not needed: {reason}")),
        Outcome::AlreadyDone => Some(format!("already set up: {reason}")),
        Outcome::Done => Some(reason.clone()),
    };
    if let Some(message) = message {
        output::success(&message);
    }
    if let Some(hint) = &report.hint {
        let _ = writeln!(anstream::stderr(), "  {hint}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_shows_steps_profile_script_and_command() {
        let target = PathBuf::from("/usr/local/bin/ferry");
        let plan = InstallPlan {
            exe: "/home/you/.cargo/bin/ferry".into(),
            profile_path: "/etc/apparmor.d/usr.local.bin.ferry".into(),
            profile: crate::doctor::apparmor_profile(&target),
            target,
            install: true,
            remove: vec!["/etc/apparmor.d/ferry".into()],
            parser: "/usr/sbin/apparmor_parser".into(),
            warnings: Vec::new(),
        };
        let report = Report {
            steps: plan.steps(),
            command: plan.sudo_argv(42, false),
            ..Report::new(
                "install",
                Outcome::Planned,
                "probe: setgroups failed".into(),
            )
        };
        let mut out = Vec::new();
        write_install_plan(&mut out, &report, &plan);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("(probe: setgroups failed)"), "{out}");
        assert!(
            out.contains(
                "  1. install this ferry (/home/you/.cargo/bin/ferry) as /usr/local/bin/ferry"
            ),
            "{out}"
        );
        assert!(
            out.contains("  3. remove the older Ferry profile /etc/apparmor.d/ferry"),
            "{out}"
        );
        assert!(out.contains(&plan.profile), "{out}");
        assert!(out.contains(setup::ROOT_SCRIPT), "{out}");
        assert!(
            out.contains(
                "sudo -- /bin/sh -euc \"$SCRIPT\" ferry-setup 42 /usr/local/bin/ferry \
                 /usr/sbin/apparmor_parser /etc/apparmor.d/usr.local.bin.ferry \
                 /etc/apparmor.d/ferry"
            ),
            "{out}"
        );
        assert!(
            out.contains("Undo: ferry setup apparmor --uninstall"),
            "{out}"
        );
        assert_eq!(
            display_argv(&["a b".into(), String::new()], "x"),
            "'a b' ''"
        );
    }
}
