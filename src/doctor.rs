//! `ferry doctor`: what this machine supports, what Ferry will do, and how to
//! fix what is missing.

use std::path::Path;

use serde::Serialize;

use crate::config::{SandboxMode, Settings};
use crate::context::Context;
use crate::fork;
use crate::freshness::{self, FreshnessMode};
use crate::fsutil;
use crate::git;
use crate::platform::{Capabilities, Os, Probe};
use crate::sandbox::Isolation;
use crate::toolchain::Toolchain;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    /// What to do about a warning or failure.
    pub fix: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
    pub capabilities: Option<Capabilities>,
    /// What `ferry spawn` would do with the current settings.
    pub plan: Option<Plan>,
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub backend: Option<fork::BackendKind>,
    pub backend_reason: String,
    pub isolation: Isolation,
    pub freshness: Option<FreshnessMode>,
    /// Whether an agent's first edit can reuse the base's incremental state.
    pub incremental_reuse: bool,
}

impl Report {
    pub fn has_failures(&self) -> bool {
        self.checks.iter().any(|c| c.status == Status::Fail)
    }
}

/// Runs every check. `ctx` is `Err` when the current directory is not a
/// usable repository; the machine-level checks still run.
pub fn run(ctx: Result<&Context, String>) -> Report {
    let mut checks = Vec::new();
    match git::version() {
        Ok(v) => checks.push(ok("git", v)),
        Err(e) => checks.push(fail(
            "git",
            e.to_string(),
            Some("install git 2.20 or newer".into()),
        )),
    }

    let ctx = match ctx {
        Ok(ctx) => ctx,
        Err(e) => {
            checks.push(fail(
                "repository",
                e,
                Some("run `ferry doctor` inside the git repository you want to use".into()),
            ));
            return Report {
                checks,
                capabilities: None,
                plan: None,
            };
        }
    };
    checks.push(ok(
        "repository",
        ctx.repo.main_worktree.display().to_string(),
    ));
    checks.push(match &ctx.settings.config_path {
        Some(p) => ok("config", p.display().to_string()),
        None => info(
            "config",
            "no ferry.toml; using defaults (`ferry init` writes one)".into(),
        ),
    });

    let toolchain = Toolchain::detect(&ctx.repo.main_worktree);
    let freshness = match &toolchain {
        Ok(t) => {
            checks.push(ok(
                "toolchain",
                format!("rustc {} ({}), {}", t.release, t.channel, t.cargo_version),
            ));
            freshness::resolve_mode(ctx.settings.freshness, t).ok()
        }
        Err(e) => {
            checks.push(fail(
                "toolchain",
                e.to_string(),
                Some("install Rust with rustup".into()),
            ));
            None
        }
    };
    if let Some(mode) = freshness {
        checks.push(match mode {
            FreshnessMode::Checksum => {
                ok("freshness", "checksum freshness (content hashes)".into())
            }
            FreshnessMode::Mtime => ok(
                "freshness",
                "mtime stamping of files proven identical to the base".into(),
            ),
        });
    }

    let caps = Capabilities::detect(ctx.store.root());
    checks.push(store_check(ctx.store.root(), &caps));
    checks.push(match &caps.reflink {
        Probe::Ok => ok(
            "reflink",
            format!("copy-on-write clones work on {}", caps.store_fs.name),
        ),
        other => info(
            "reflink",
            format!(
                "unavailable on {}: {}",
                caps.store_fs.name,
                probe_detail(other)
            ),
        ),
    });
    checks.extend(namespace_checks(&caps));
    match crate::base::unapplied_cargo_configs(ctx) {
        Ok(configs) => checks.extend(configs.iter().map(|config| {
            warn(
                "cargo config",
                crate::base::unapplied_message(ctx, config),
                Some(crate::base::CONFIG_ADVICE.into()),
            )
        })),
        Err(e) => checks.push(warn("cargo config", e.to_string(), None)),
    }

    let plan = plan(&ctx.settings, &caps, freshness);
    Report {
        checks,
        capabilities: Some(caps),
        plan: Some(plan),
    }
}

fn store_check(root: &Path, caps: &Capabilities) -> Check {
    let free = rustix::fs::statvfs(root)
        .map(|s| s.f_bavail.saturating_mul(s.f_frsize))
        .ok();
    let detail = format!(
        "{} ({}{})",
        root.display(),
        caps.store_fs.name,
        free.map(|b| format!(", {} free", fsutil::human_bytes(b)))
            .unwrap_or_default()
    );
    if matches!(
        caps.store_fs.name.as_str(),
        "nfs" | "cifs" | "smb2" | "fuse"
    ) {
        return warn(
            "store",
            detail,
            Some("network and FUSE filesystems make builds slow and mtimes unreliable; set FERRY_HOME to a local disk".into()),
        );
    }
    ok("store", detail)
}

fn namespace_checks(caps: &Capabilities) -> Vec<Check> {
    if caps.os == Os::Macos {
        return vec![info(
            "sandbox",
            "macOS has no mount namespaces: agents build at their own paths (dependencies reused, \
             workspace crates rebuild once, no incremental reuse on the first edit)"
                .into(),
        )];
    }
    let mut out = Vec::new();
    match &caps.userns {
        Probe::Ok => out.push(ok(
            "namespaces",
            "unprivileged user + mount namespaces work".into(),
        )),
        other => out.push(warn(
            "namespaces",
            probe_detail(other),
            Some(userns_fix(caps)),
        )),
    }
    match &caps.overlay {
        Probe::Ok => out.push(ok("overlayfs", "mounts inside user namespaces".into())),
        other if caps.userns.is_ok() => out.push(warn(
            "overlayfs",
            probe_detail(other),
            Some("overlayfs needs Linux 5.11+ and a store filesystem with user xattrs (ext4, btrfs, xfs; tmpfs on 6.6+)".into()),
        )),
        _ => {}
    }
    out
}

fn userns_fix(caps: &Capabilities) -> String {
    if caps.sysctl("kernel.apparmor_restrict_unprivileged_userns") == Some("1") {
        return "Ubuntu's AppArmor policy denies capabilities in unprivileged user namespaces. Install \
                ferry to a root-owned path (e.g. /usr/local/bin/ferry) and load the profile it prints: \
                `/usr/local/bin/ferry doctor --apparmor-profile | sudo tee /etc/apparmor.d/ferry && \
                sudo apparmor_parser -r /etc/apparmor.d/ferry` (see the troubleshooting guide)"
            .into();
    }
    if caps.sysctl("kernel.unprivileged_userns_clone") == Some("0") {
        return "unprivileged user namespaces are disabled: `sudo sysctl kernel.unprivileged_userns_clone=1`".into();
    }
    if caps.sysctl("user.max_user_namespaces") == Some("0") {
        return "user namespaces are disabled: `sudo sysctl user.max_user_namespaces=15000`".into();
    }
    "if this runs in a container, allow unshare(CLONE_NEWUSER) (e.g. a seccomp profile permitting it), \
     or set FERRY_SANDBOX=off"
        .into()
}

fn plan(settings: &Settings, caps: &Capabilities, freshness: Option<FreshnessMode>) -> Plan {
    let isolation = if settings.sandbox != SandboxMode::Off && caps.userns.is_ok() {
        Isolation::Namespace
    } else {
        Isolation::None
    };
    let (backend, backend_reason) = match fork::select(settings.backend, caps, isolation) {
        Ok(choice) => (Some(choice.kind), choice.reason),
        Err(e) => (None, e.to_string()),
    };
    Plan {
        backend,
        backend_reason,
        isolation,
        freshness,
        incremental_reuse: isolation == Isolation::Namespace,
    }
}

/// An `AppArmor` profile that lets `binary` create user namespaces on Ubuntu
/// 24.04+ (`kernel.apparmor_restrict_unprivileged_userns=1`).
///
/// It is the stacked form Ubuntu uses for `bwrap-userns-restrict`: ferry
/// itself may create the namespace and mount, while everything it executes
/// runs under `unpriv_ferry`, which denies capabilities. The simpler
/// `flags=(unconfined)` profile would let every process an agent starts
/// create privileged user namespaces, exempting the whole agent from the
/// restriction.
pub fn apparmor_profile(binary: &Path) -> String {
    let path = binary.display();
    format!(
        "# AppArmor profile: lets {path} create unprivileged user namespaces.\n\
         # The path must be root-owned and not writable by users.\n\
         # Install: sudo cp this-file /etc/apparmor.d/ferry && sudo apparmor_parser -r /etc/apparmor.d/ferry\n\
         abi <abi/4.0>,\n\
         include <tunables/global>\n\
         \n\
         profile ferry {path} flags=(attach_disconnected) {{\n\
         \x20 allow capability,\n\
         \x20 allow file rwlkm /{{**,}},\n\
         \x20 allow network,\n\
         \x20 allow unix,\n\
         \x20 allow ptrace,\n\
         \x20 allow signal,\n\
         \x20 allow mqueue,\n\
         \x20 allow io_uring,\n\
         \x20 allow userns,\n\
         \x20 allow mount,\n\
         \x20 allow umount,\n\
         \x20 allow pivot_root,\n\
         \x20 allow dbus,\n\
         \x20 allow px /** -> ferry//&unpriv_ferry,\n\
         \n\
         \x20 include if exists <local/ferry>\n\
         }}\n\
         \n\
         profile unpriv_ferry flags=(attach_disconnected) {{\n\
         \x20 allow file rwlkm /{{**,}},\n\
         \x20 allow network,\n\
         \x20 allow unix,\n\
         \x20 allow ptrace,\n\
         \x20 allow signal,\n\
         \x20 allow mqueue,\n\
         \x20 allow io_uring,\n\
         \x20 allow userns,\n\
         \x20 allow mount,\n\
         \x20 allow umount,\n\
         \x20 allow pivot_root,\n\
         \x20 allow dbus,\n\
         \x20 allow pix /** -> &unpriv_ferry,\n\
         \x20 audit deny capability,\n\
         \n\
         \x20 include if exists <local/unpriv_ferry>\n\
         }}\n"
    )
}

/// Why `binary` is unsuitable for an `AppArmor` profile, if it is: the
/// profile attaches by path, so anyone who can replace the file inherits the
/// exemption.
pub fn apparmor_path_problem(binary: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(binary).ok()?;
    let dir_meta = binary.parent().and_then(|d| std::fs::metadata(d).ok())?;
    let user_writable = |m: &std::fs::Metadata| m.uid() != 0 || m.mode() & 0o022 != 0;
    (user_writable(&meta) || user_writable(&dir_meta)).then(|| {
        format!(
            "{} is writable by a non-root user; install ferry to a root-owned path first, e.g. \
             `sudo install -m 755 {} /usr/local/bin/ferry`, then run `/usr/local/bin/ferry doctor --apparmor-profile`",
            binary.display(),
            binary.display()
        )
    })
}

fn probe_detail(probe: &Probe) -> String {
    match probe {
        Probe::Ok => "ok".into(),
        Probe::Unsupported { reason } => reason.clone(),
        Probe::Failed { stage, error } => format!("{stage} failed: {error}"),
    }
}

fn ok(name: &'static str, detail: String) -> Check {
    Check {
        name,
        status: Status::Ok,
        detail,
        fix: None,
    }
}

fn info(name: &'static str, detail: String) -> Check {
    Check {
        name,
        status: Status::Info,
        detail,
        fix: None,
    }
}

fn warn(name: &'static str, detail: String, fix: Option<String>) -> Check {
    Check {
        name,
        status: Status::Warn,
        detail,
        fix,
    }
}

fn fail(name: &'static str, detail: String, fix: Option<String>) -> Check {
    Check {
        name,
        status: Status::Fail,
        detail,
        fix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apparmor_profile_is_stacked_and_names_the_binary() {
        let p = apparmor_profile(Path::new("/usr/local/bin/ferry"));
        assert!(p.contains("profile ferry /usr/local/bin/ferry flags=(attach_disconnected) {"));
        assert!(p.contains("  allow userns,"));
        assert!(p.contains("  allow px /** -> ferry//&unpriv_ferry,"));
        assert!(p.contains("profile unpriv_ferry flags=(attach_disconnected) {"));
        assert!(p.contains("  audit deny capability,"));
        assert!(p.contains("  allow file rwlkm /{**,},"));
        assert!(p.contains("abi <abi/4.0>,"));
        assert_eq!(p.matches('{').count(), p.matches('}').count());
    }

    #[test]
    fn user_writable_binary_is_rejected_for_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("ferry");
        std::fs::write(&bin, b"").unwrap();
        assert!(apparmor_path_problem(&bin).is_some());
    }

    #[test]
    fn shipped_profile_matches_generator() {
        let shipped = include_str!("../contrib/apparmor/ferry");
        assert_eq!(shipped, apparmor_profile(Path::new("/usr/local/bin/ferry")));
    }
}
