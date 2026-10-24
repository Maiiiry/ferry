//! `ferry doctor`: what this machine supports, what Ferry will do, and how to
//! fix what is missing.

use std::path::Path;

use serde::Serialize;

use crate::config::{BackendPref, SandboxMode, Settings};
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
                format!("rustc {} ({:?}), {}", t.release, t.channel, t.cargo_version),
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
            "macOS has no mount namespaces: agents build at their own paths (crate-level reuse, \
             no incremental reuse on the first edit)"
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
        return "Ubuntu's AppArmor policy blocks unprivileged user namespaces. Install a profile for the \
                ferry binary: `ferry doctor --apparmor-profile | sudo tee /etc/apparmor.d/ferry && \
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

/// An AppArmor profile that lets `binary` create user namespaces on Ubuntu
/// 23.10+ (`kernel.apparmor_restrict_unprivileged_userns=1`), in the same
/// form Ubuntu ships for other sandboxing tools.
pub fn apparmor_profile(binary: &Path) -> String {
    format!(
        "# AppArmor profile allowing ferry to create unprivileged user namespaces.\n\
         # Install: sudo tee /etc/apparmor.d/ferry < this-file && sudo apparmor_parser -r /etc/apparmor.d/ferry\n\
         abi <abi/4.0>,\n\
         include <tunables/global>\n\
         \n\
         profile ferry {} flags=(unconfined) {{\n\
         \x20 userns,\n\
         \n\
         \x20 include if exists <local/ferry>\n\
         }}\n",
        binary.display()
    )
}

/// Default backend preference text for messages.
pub fn backend_pref_name(pref: BackendPref) -> String {
    pref.to_string()
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
    fn apparmor_profile_names_the_binary() {
        let p = apparmor_profile(Path::new("/home/u/.cargo/bin/ferry"));
        assert!(p.contains("profile ferry /home/u/.cargo/bin/ferry flags=(unconfined) {"));
        assert!(p.contains("  userns,"));
        assert!(p.contains("abi <abi/4.0>,"));
    }
}
