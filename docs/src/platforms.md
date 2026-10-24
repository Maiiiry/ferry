# Platform support

Ferry gives every agent a warm, private target dir everywhere it runs. What
varies is whether an agent's **first edit** can reuse rustc's incremental
state, which needs the Linux namespace sandbox (or the experimental
[remap-paths mode](remap-paths.md)).

- **Crate-level reuse**: crates whose sources match the base are not rebuilt.
  An edit rebuilds the edited crate and its dependents from scratch.
- **Incremental reuse**: on top of that, rustc recompiles only the parts of an
  edited crate that changed. In the prototype on ripgrep (2 vCPU) the first
  edit after spawning took 1.75 s with incremental reuse and 5.3 s with
  crate-level reuse only.

After an agent's first build, its own incremental state exists and later
edits are incremental either way.

## Summary

| Platform | Fork backends | Sandbox | First edit after spawn |
|---|---|---|---|
| Linux with unprivileged user namespaces (Debian, Fedora, Arch, NixOS, RHEL without STIG hardening, ...) | overlay, reflink, copy | namespace sandbox with canonical paths | incremental reuse |
| Ubuntu 24.04 and later, with Ferry's AppArmor profile installed | overlay, reflink, copy | namespace sandbox | incremental reuse |
| Ubuntu 24.04 and later, default policy | reflink, copy | none | crate-level reuse |
| Docker, default seccomp profile | reflink, copy | none | crate-level reuse |
| macOS (APFS) | reflink (`clonefile`), copy | none | crate-level reuse; experimental remap-paths mode |
| Windows | not supported | | |

Ferry does not build on Windows: compiling it there stops with a
`compile_error!` saying so.

## Linux

The full feature set: agents build in a private user and mount namespace, at
canonical paths, and the overlay backend forks in constant time. This needs:

- unprivileged user namespaces, which are on by default on Debian 12 and 13,
  Fedora, RHEL 9 and 10, Arch and NixOS, but not on Ubuntu 24.04 and later
  (AppArmor) or in Docker's default seccomp profile, and are disabled by the
  DISA STIG for RHEL (`user.max_user_namespaces = 0`);
- for the overlay backend, Linux 5.11 or newer and a store filesystem that
  supports user extended attributes (ext4, btrfs, XFS; tmpfs from 6.6).

When namespaces are unavailable, `sandbox = "auto"` falls back to building at
host paths, and `ferry base` warns about it. See
[Troubleshooting](troubleshooting.md#user-namespaces) for every case.

### Ubuntu 24.04 and later

Ubuntu restricts unprivileged user namespaces with AppArmor
(`kernel.apparmor_restrict_unprivileged_userns = 1`). Creating one succeeds,
but the process gets no capabilities inside it, so mapping its uid and
mounting fail. Ferry detects this and either:

- uses the namespace sandbox once you install the AppArmor profile from
  `ferry doctor --apparmor-profile` for a root-owned `ferry` binary (CI runs
  Ferry's sandbox tests on Ubuntu 24.04 this way), or
- falls back to crate-level reuse.

See [AppArmor on Ubuntu](troubleshooting.md#apparmor-on-ubuntu-2404-and-later).
Ubuntu 23.10 ships the restriction switched off.

### Containers

Docker's default seccomp profile allows `unshare` and `mount` only with
`CAP_SYS_ADMIN`, so Ferry falls back to crate-level reuse inside Docker.
Rootless Podman's default profile allows them. If you run Ferry in a
container with namespaces allowed, the store must not be on the container's
overlay root filesystem, because overlayfs cannot use another overlayfs as its
upper layer: put it on a volume or a tmpfs. These container facts come from
the profiles' sources and probes during Ferry's research; Ferry itself has not
been tested in containers.

### Filesystems

Where the store lives decides which fork backends work and what they cost.

| Filesystem | reflink backend | overlay upper layer | Notes |
|---|---|---|---|
| btrfs | yes | yes | overlay copy-up is a reflink |
| XFS | yes (`reflink=1`, the mkfs default since xfsprogs 5.1) | expected to work, not tested | overlay copy-up is a reflink |
| OpenZFS | 2.2.0, and 2.3.0 or newer; off by default in 2.2.2 to 2.2.x | not tested | |
| ext4 | no | yes | overlay copy-up copies the whole file |
| tmpfs | no | Linux 6.6 or newer | |
| NFS, CIFS, FUSE | not recommended | not recommended | `ferry doctor` warns: slow builds, unreliable mtimes |

`ferry doctor` probes reflinks and overlayfs on the store's actual filesystem
rather than trusting this table.

## macOS

Agents fork the base's target dir with one `clonefile(2)` call on APFS (the
`reflink` backend) and build at their own paths. macOS has no per-process
mount namespaces, so there is no canonical path: crate-level reuse works, and
the first edit of a crate loses its incremental state.

The experimental [remap-paths mode](remap-paths.md) is designed for this
case. On Linux it reuses 12 of 13 library codegen units after a fork to a new
path, against 0 of 13 without it. That it works the same on macOS is predicted
from rustc's source but has not been verified on a Mac.

Bases and forks live in `~/Library/Caches/ferry`; Ferry excludes base target
dirs and agent build state from Time Machine backups. Agent worktrees live in
`~/Library/Application Support/ferry/worktrees`, because macOS may purge
caches.

## What `ferry doctor` reports

`ferry doctor` runs these checks and ends with the plan for new agents.

| Check | Status | What it means |
|---|---|---|
| `git` | ok or fail | `git --version`; fails if git is missing |
| `repository` | ok or fail | the main checkout Ferry will use; fails outside a git repository |
| `config` | ok or info | the `ferry.toml` that was loaded, or that defaults are in use |
| `toolchain` | ok or fail | the `rustc` and `cargo` the repository selects |
| `freshness` | ok | `checksum freshness (content hashes)` or `mtime stamping of files proven identical to the base` |
| `store` | ok or warn | the store path, its filesystem and free space; warns on NFS, CIFS and FUSE |
| `reflink` | ok or info | whether copy-on-write clones work on the store's filesystem |
| `namespaces` | ok or warn (Linux) | whether an unprivileged user and mount namespace with a bind mount works; the fix names the cause it found (AppArmor, a sysctl, or a container) |
| `overlayfs` | ok or warn (Linux) | whether overlayfs mounts inside such a namespace, with upper and work dirs in the store |
| `sandbox` | info (macOS) | that macOS has no mount namespaces |

The plan line reads like:

```text
  → agents fork with overlay (overlayfs works in the namespace sandbox on btrfs) and build in a namespace sandbox at canonical paths (incremental reuse on the first edit)
```

or, without namespaces:

```text
  → agents fork with reflink (btrfs supports copy-on-write clones) and build at their own paths (crate-level reuse only)
```

`ferry --json doctor` adds the raw probe results and the sysctls that explain
namespace failures: `user.max_user_namespaces`,
`kernel.unprivileged_userns_clone`,
`kernel.apparmor_restrict_unprivileged_userns`,
`kernel.apparmor_restrict_unprivileged_unconfined` and
`kernel.yama.ptrace_scope`, where they exist.
