# Linux user/mount namespaces, overlayfs, reflinks, AppArmor: research brief

Date: 2026-10-23. Scope: what Ferry needs to know to run builds in a private user+mount namespace
with an overlayfs (or reflink) target dir at a canonical path.

Evidence tags used below:

- **[src]** read in primary source (kernel/man/docs/package source; link given).
- **[local]** measured on this box: NixOS, kernel 6.18.55, btrfs `/home`, tmpfs `/tmp`, util-linux 2.42.3,
  coreutils 9.11, Yama `ptrace_scope=1`, LSMs `capability,landlock,yama,bpf,ima` (no AppArmor).
- **[vm]** measured in a stock Ubuntu 24.04.5 cloud VM (kernel 6.8.0-147-generic, apparmor
  4.0.1really4.0.1-0ubuntu0.24.04.9), as an unprivileged user.
- **[unverified]** could not confirm; treat as a lead, not a fact.

Experiment scripts and raw outputs: `$SCRATCH/research-ns/` (exp*.sh, `rxtest/` Rust harness, `vm/intest*.{sh,out}`).

---

## TL;DR

1. Unprivileged userns is **on by default everywhere relevant** (Debian 12/13, Fedora, RHEL 9/10, Arch, NixOS,
   WSL2) **except Ubuntu ≥ 24.04** (AppArmor restriction on by default) and **Docker's default seccomp**
   (blocks `unshare`/`mount`/`setns` without CAP_SYS_ADMIN). STIG-hardened RHEL sets `user.max_user_namespaces=0`.
2. On Ubuntu 24.04+ `unshare(CLONE_NEWUSER)` **succeeds**, but the process is relabelled `unprivileged_userns`
   and every capability check fails: the **first write to `/proc/self/{setgroups,uid_map}` fails (EPERM/EACCES)**,
   and mounts are denied. A per-binary AppArmor profile (`userns,`) fixes it; verified in the VM, including a
   bwrap-style stacked profile that still runs `cargo build`/`cargo test`/`git commit` but keeps children capability-less.
3. Overlayfs in a userns needs kernel **≥ 5.11** and **must** use `-o userxattr` (without it, `rmdir` of a lower dir
   fails with EIO). In a userns, redirect_dir/metacopy/index/nfs_export are **always off**, so renaming a lower dir
   gives **EXDEV**, and the kernel's "upperdir in use" protection is **only a warning**: two mounts on one upper
   succeed and give inconsistent views (measured). **Ferry must enforce one-mount-per-upper itself.**
4. **Copy-up is a reflink when upper and lower are on the same btrfs/XFS filesystem**: 64 MiB copy-up in 1.4 ms with
   0 exclusive bytes, vs 44 ms + 65 MB with a tmpfs upper. This removes most of the ~190 MB/agent upper cost
   in RESULTS.md (which was on ext4). rustc's incremental session setup hard-links every previous-session file,
   and each `link()` of a lower file forces a copy-up.
5. Overlay mount is ~10 ms; `cp -a --reflink=always` of 10k small files is ~490 ms and scales with file count,
   not bytes. Overlay is the better default fork mechanism where namespaces work; reflink-copy is the fallback.
6. Rejoining a live sandbox works unprivileged: `pidfd_open` + `setns(USER|MOUNT)` from a forked,
   single-threaded child. Yama does not interfere. setns resets cwd to `/`, after std has already applied
   `current_dir`.

---

## 1. Unprivileged user namespaces: availability matrix

Kernel default for `user.max_user_namespaces` is `max_threads/2` ([src fork.c](https://github.com/torvalds/linux/blob/v6.18/kernel/fork.c#L832-L833); 127301 here [local]).

| Platform | Mechanism / default | Evidence |
|---|---|---|
| Ubuntu 23.10 | AppArmor restriction **available but off**: `10-apparmor.conf` sets `kernel.apparmor_restrict_unprivileged_userns = 0` | [src apparmor 4.0.0~alpha2-0ubuntu5](https://git.launchpad.net/ubuntu/+source/apparmor/tree/debian/usr/lib/sysctl.d/10-apparmor.conf?h=ubuntu/mantic); [23.10 notes](https://documentation.ubuntu.com/release-notes/23.10/) "not currently enabled by default" |
| Ubuntu 24.04 | `restrict_unprivileged_userns = 1`; `restrict_unprivileged_unconfined` not set by the sysctl file (VM reads **0**) | [src noble-updates](https://git.launchpad.net/ubuntu/+source/apparmor/tree/debian/usr/lib/sysctl.d/10-apparmor.conf?h=ubuntu/noble-updates), [24.04 notes](https://documentation.ubuntu.com/release-notes/24.04/), [vm] |
| Ubuntu 24.10 | `restrict_unprivileged_userns = 1` only | [src oracular](https://git.launchpad.net/ubuntu/+source/apparmor/tree/debian/usr/lib/sysctl.d/10-apparmor.conf?h=ubuntu/oracular) |
| Ubuntu 25.04 / 25.10 / 26.04 / 26.10 | both `= 1` | [src](https://git.launchpad.net/ubuntu/+source/apparmor/tree/debian/usr/lib/sysctl.d/10-apparmor.conf?h=ubuntu/resolute) branches plucky-updates, questing-updates, resolute, stonking |
| Debian 12 / 13 | Debian kernel patch keeps `kernel.unprivileged_userns_clone`, default **1** ("change the default to enabled") | [src trixie patch](https://salsa.debian.org/kernel-team/linux/-/blob/debian/6.12/trixie/debian/patches/debian/add-sysctl-to-disallow-unprivileged-CLONE_NEWUSER-by-default.patch), same in bookworm |
| Fedora, RHEL 9/10 lineage | `CONFIG_USER_NS=y` in kernel-ark common config; no extra sysctl. **DISA STIG V-257816 requires `user.max_user_namespaces = 0`** | [src kernel-ark](https://gitlab.com/cki-project/kernel-ark/-/blob/os-build/redhat/configs/common/generic/CONFIG_USER_NS), [STIG](https://www.stigviewer.com/stigs/red_hat_enterprise_linux_9/2025-02-27/finding/V-257816) |
| Arch (`linux` 7.2.11) | `CONFIG_USER_NS=y`, no `unprivileged_userns_clone` patch, AppArmor not in default `CONFIG_LSM` | [src config.x86_64](https://gitlab.archlinux.org/archlinux/packaging/packages/linux/-/blob/main/config.x86_64) |
| NixOS | `security.allowUserNamespaces` default `true` (false sets `user.max_user_namespaces=0`) | [src misc.nix](https://github.com/NixOS/nixpkgs/blob/master/nixos/modules/security/misc.nix), [local] works |
| WSL2 (kernel 6.18.y) | `CONFIG_USER_NS=y`, `CONFIG_OVERLAY_FS=y`; AppArmor built in, but the Ubuntu-only `restrict_unprivileged_userns` sysctl does not exist in non-Ubuntu kernels (see §2) | [src config-wsl](https://github.com/microsoft/WSL2-Linux-Kernel/blob/linux-msft-wsl-6.18.y/arch/x86/configs/config-wsl); WSL runtime behavior [unverified] |
| Docker (default seccomp) | `unshare`, `mount`, `setns`, `fsopen`… allowed **only with CAP_SYS_ADMIN**; `clone` with ns flags blocked; `clone3` → ENOSYS | [src moby/profiles default.json](https://github.com/moby/profiles/blob/main/seccomp/default.json); [local] `unshare: Operation not permitted` as root and uid 1000 |
| Docker `--security-opt seccomp=unconfined` | `unshare -Urm` + overlay works **if upper is not on the container's overlay rootfs** (`--tmpfs` or volume) | [local] Docker 29.8.1 on a non-AppArmor host |
| Podman rootless (default) | `unshare`/`mount` allowed unconditionally; `unshare -Urm` + overlay(tmpfs upper) works | [src containers/common seccomp.json](https://github.com/containers/common/blob/main/pkg/seccomp/seccomp.json), [local] Podman 5.8.8 |
| GitHub Actions `ubuntu-24.04` = `ubuntu-latest` | Stock Ubuntu 24.04 policy: runner-images scripts never touch the apparmor sysctls; flatpak's CI on these runners does `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` | [src runner-images README @c87f6f6](https://github.com/actions/runner-images/blob/main/README.md) (ubuntu-latest → 24.04; 26.04 available), [flatpak check.yml](https://github.com/flatpak/flatpak/blob/main/.github/workflows/check.yml#L30-L32); runtime value itself [unverified] |

Other `unshare(CLONE_NEWUSER)` failure codes [src [unshare(2)](https://man7.org/linux/man-pages/man2/unshare.2.html)]: **ENOSPC** when
`max_user_namespaces` is exhausted or 0, **EPERM** when called from a chroot, **EINVAL** from a multithreaded
process ([local] Rust harness with 2 threads → EINVAL).

---

## 2. Ubuntu AppArmor restriction

**Mechanism.** The `apparmor_restrict_unprivileged_userns{,_force,_complain}` sysctls are **Ubuntu kernel patches**.
Mainline 6.18 has only `apparmor_restrict_unprivileged_unconfined` and mediates userns creation for confined profiles
only ([src mainline lsm.c](https://github.com/torvalds/linux/blob/v6.18/security/apparmor/lsm.c#L1055-L1074)). In
Ubuntu's kernel, `apparmor_userns_create` replaces the new cred's label with the result of the profile's userns rule
([src noble lsm.c L1245-1275](https://git.launchpad.net/~ubuntu-kernel/ubuntu/+source/linux/+git/noble/tree/security/apparmor/lsm.c)).
For unconfined callers, the new label is the `unprivileged_userns` profile: `audit deny capability, audit deny change_profile`,
everything else allowed ([src profile](https://git.launchpad.net/ubuntu/+source/apparmor/tree/profiles/apparmor.d/unprivileged_userns?h=ubuntu/noble-updates)).
If that profile is not loaded, creation is denied outright (`error=-13`) ([AppArmor wiki](https://gitlab.com/apparmor/apparmor/-/wikis/unprivileged_userns_restriction)).

**Exactly what fails [vm]:**

| Step | Result on stock 24.04 |
|---|---|
| `unshare(CLONE_NEWUSER)` | **succeeds**; `/proc/self/attr/current` = `unprivileged_userns (enforce)`; audit `Userns create - transitioning profile` |
| write `/proc/self/setgroups` (Rust harness) | **EACCES** |
| write `/proc/self/uid_map` (`unshare -Ur`) | **EPERM**: `unshare: write failed /proc/self/uid_map: Operation not permitted` (map_write needs CAP_SYS_ADMIN in the target ns, [src](https://github.com/torvalds/linux/blob/v6.8/kernel/user_namespace.c#L974-L978)); audit `capable … capname="sys_admin"` |
| `mount` in the ns (no map) | denied: audit `operation="mount" … error=-13 profile="unprivileged_userns"` |
| same binary copied to `/usr/local/bin`, no profile | same EPERM (attachment is by path) |
| `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` | plain `unshare -Urm` + mount works, label `unconfined` |

**Reliable detection from an unprivileged process.** Probe; don't infer:
1. Read `/proc/sys/kernel/apparmor_restrict_unprivileged_userns`, which is **mode 0644** on Ubuntu ([src noble lsm.c L2437-2444](https://git.launchpad.net/~ubuntu-kernel/ubuntu/+source/linux/+git/noble/tree/security/apparmor/lsm.c); [vm]). Absent means no Ubuntu restriction.
2. In a forked child, `unshare(USER|NS)`, then read `/proc/self/attr/current`: a value starting with `unprivileged_userns` is decisive.
3. Then run the real steps (setgroups → uid_map → gid_map → mount) and report the first that fails with its errno.
   `CapEff` is not a usable signal.

**Recommended profile.** It uses the same shape Ubuntu ships for podman/chrome/code/tup ([src noble podman profile](https://git.launchpad.net/ubuntu/+source/apparmor/tree/profiles/apparmor.d/podman?h=ubuntu/noble-updates)). Install as
`/etc/apparmor.d/ferry`, load with `sudo apparmor_parser -r /etc/apparmor.d/ferry` (persists across boots via apparmor.service).
Verified working [vm] for a binary at `/usr/local/bin/ferry`:

```
abi <abi/4.0>,
include <tunables/global>

profile ferry /usr/local/bin/ferry flags=(unconfined) {
  userns,

  include if exists <local/ferry>
}
```

What it grants [vm]: everything exec'd by ferry **keeps the `ferry (unconfined)` label**, so any agent process
(build.rs, test binaries, arbitrary agent shell commands) can itself create a fully privileged userns (`unshare -Ur`
inside: rc=0). That exempts the agent's whole process tree from the restriction. On 24.04, with
`restrict_unprivileged_unconfined=0`, **any** unconfined process can also `aa-exec -p ferry -- unshare -Ur` (measured
rc=0). Ubuntu ≥ 25.04 sets that sysctl to 1, which blocks this ([Ubuntu discourse](https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007)).
The same applies to every unconfined profile Ubuntu already ships (chrome, podman, …).

**Hardened alternative (verified [vm]):** modeled on Ubuntu's
[`bwrap-userns-restrict`](https://git.launchpad.net/ubuntu/+source/apparmor/tree/profiles/apparmor/profiles/extras/bwrap-userns-restrict?h=ubuntu/noble-updates).
ferry keeps its capabilities; everything it execs is stacked with a capability-denying profile.

```
abi <abi/4.0>,
include <tunables/global>

profile ferry /usr/local/bin/ferry flags=(attach_disconnected) {
  allow capability,
  allow file rwlkm /{**,},
  allow network,
  allow unix,
  allow ptrace,
  allow signal,
  allow mqueue,
  allow io_uring,
  allow userns,
  allow mount,
  allow umount,
  allow pivot_root,
  allow dbus,
  allow px /** -> ferry//&unpriv_ferry,

  include if exists <local/ferry>
}

profile unpriv_ferry flags=(attach_disconnected) {
  allow file rwlkm /{**,},
  allow network,
  allow unix,
  allow ptrace,
  allow signal,
  allow mqueue,
  allow io_uring,
  allow userns,
  allow mount,
  allow umount,
  allow pivot_root,
  allow dbus,
  allow pix /** -> &unpriv_ferry,
  audit deny capability,

  include if exists <local/unpriv_ferry>
}
```

This is the exact text loaded in the VM. Result [vm]:
- ferry's in-process unshare, uid_map, fsopen overlay and bind succeed.
- Children are labelled `ferry//&unpriv_ferry (enforce)`, and their `unshare -Ur` fails (`capname="sys_admin"` denied).
- `cargo build`, `cargo test` (with build.rs), `git commit` and `git worktree list` all work at the canonical path with no denials.

**Sysctl-off alternative** (`/etc/sysctl.d/60-apparmor-namespace.conf: kernel.apparmor_restrict_unprivileged_userns=0`,
[24.04 notes](https://documentation.ubuntu.com/release-notes/24.04/)). This re-opens unprivileged userns for **every process on
the machine**, including browser renderers and anything an attacker runs. That is the attack surface the restriction exists
to close: the AppArmor wiki lists CVE-2022-0185 and CVE-2024-1086 among exploits reachable via userns caps. The profile limits the
exemption to one root-owned path. Never attach a profile to a user-writable path such as `~/.cargo/bin/ferry`: anyone able to write
there gets the exemption.

---

## 3. Overlayfs inside an unprivileged user namespace

Kernel source cited at v6.18 unless noted.

- **Minimum kernel 5.11**: `FS_USERNS_MOUNT` and `userxattr` are both absent in v5.10 and present in v5.11 ([super.c v5.11](https://github.com/torvalds/linux/blob/v5.11/fs/overlayfs/super.c)) [src].
- **`userxattr` is not implied.** Without it, the mount succeeds but logs `failed to set xattr on upper … try mounting with 'userxattr' option`
  and runs with no xattrs (`redirect_dir=nofollow,uuid=null`) [local journalctl -k]. Consequence [local]: `rm -rf` of a
  lower subdirectory then fails with **EIO**. **Always pass `userxattr`.**
- **Features in a userns** ([params.c L965-1013](https://github.com/torvalds/linux/blob/v6.18/fs/overlayfs/params.c#L965-L1013), [util.c ovl_can_decode_fh](https://github.com/torvalds/linux/blob/v6.18/fs/overlayfs/util.c#L85-L94)):

| Feature | In userns + userxattr | Measured [local] |
|---|---|---|
| redirect_dir | forced `nofollow`; explicit `on`/`follow` → EINVAL ("conflicting options") | rename(2) of a lower dir → **EXDEV** |
| metacopy | forced off; explicit `on` → EINVAL (EPERM without userxattr) | full-data copy-up on chmod/utimes |
| index, nfs_export | silently off: file handles need CAP_DAC_READ_SEARCH in init ns | `index=on` accepted, not shown in mountinfo |
| xino | `xino=on` "useless with all layers on same fs, ignore" | with a tmpfs upper, merged inode numbers come from two different filesystems |
| volatile | allowed (since 5.10) | see below |

- **Directory semantics [local, btrfs and tmpfs upper]**:
  - Renaming a lower file works (it is copied up).
  - Renaming a dir created in the upper works. Renaming a lower/merged dir gives EXDEV (`mv` silently falls back to copying).
  - `rm -rf lowerdir/sub && mkdir sub` gives an empty dir marked `user.overlay.opaque="y"`.
  - Whiteouts are 0:0 char devices, created fine in a userns (`vfs_mknod` exempts `WHITEOUT_DEV` from CAP_MKNOD since 5.8, [namei.c](https://github.com/torvalds/linux/blob/v6.18/fs/namei.c#L4300-L4308)).
- **Hardlinks**: `link()` of a lower file copies it up first ([docs](https://github.com/torvalds/linux/blob/master/Documentation/filesystems/overlayfs.rst)). With index off,
  copying up a file that already had several links breaks the link set. Measured: after `ln lower/hl hl2`, the two names show
  **different `st_ino`** in the merged view (lower-origin ino vs upper ino) although both have nlink=2.
- **Copy-up triggers**:
  - Any open for write copies up, even `O_RDWR` with no writes ([local]).
  - `utimes` copies up ([local]); chmod copies up ([docs]).
  - `open(O_TRUNC)` copies up with size 0, i.e. no data copy ([copy_up.c L1185](https://github.com/torvalds/linux/blob/v6.18/fs/overlayfs/copy_up.c#L1185-L1187); [local] size=0).
  - The data copy tries `vfs_clone_file_range` first ([copy_up.c L283](https://github.com/torvalds/linux/blob/v6.18/fs/overlayfs/copy_up.c#L283-L286)).
- **Upper filesystems**:
  - Work: btrfs, tmpfs [local]; ext4 [vm, 6.8].
  - tmpfs needs ≥ 6.6 for `user.*` xattrs (`shmem_user_xattr_handler` absent in v6.5, present in v6.6 [src]).
  - overlayfs can **never** be an upper (`DCACHE_OP_REAL` check, [params.c L308-310](https://github.com/torvalds/linux/blob/v6.18/fs/overlayfs/params.c#L308-L310)). This is why the upper failed inside Docker/Podman on the container rootfs [local]. A lower on overlayfs is allowed.
  - XFS upper: not tested.
- **volatile**:
  - Skips all syncs and creates `$workdir/work/incompat/volatile`.
  - The **next mount fails even after a clean umount** ("overlay with incompat feature 'volatile' cannot be mounted") until that dir is removed [local].
  - `fsync=auto|strict|volatile` is mainline 7.0, and is present in 6.18.55 stable ([src](https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/overlayfs/params.c?h=v6.18.55#n176)).
- **Two mounts on the same upper/workdir** (in different mount namespaces):
  - The in-use flag is per inode (`I_OVL_INUSE`), so it is global across namespaces.
  - With `index=on`: EBUSY. With index off (always in a userns): only `pr_warn` ([super.c L488-499](https://github.com/torvalds/linux/blob/v6.18/fs/overlayfs/super.c#L488-L499)), and the doc calls the result undefined.
  - [local]: both mounts succeeded. The kernel logged `upperdir is in-use … undefined behavior`. Mount A then still listed a file B had deleted and read the old content of a file B had rewritten, but saw B's new file.
- **Option escaping and limits**:
  - Legacy `mount(2)`: options split on `,` with `\` escaping ([ovl_next_opt](https://github.com/torvalds/linux/blob/v6.18/fs/overlayfs/params.c#L163-L184)); lowerdir uses `:` separators (escape `\:`).
  - Legacy data is capped at one page ([copy_mount_options](https://github.com/torvalds/linux/blob/v6.18/fs/namespace.c#L3898-L3906)).
  - `fsconfig(FSCONFIG_SET_STRING)` values are capped at **256 bytes** ([fsopen.c L430](https://github.com/torvalds/linux/blob/v6.18/fs/fsopen.c#L428-L435); [local] 287-byte path → EINVAL).
  - `lowerdir+`/`datadir+` (6.8+) take literal paths; [local]: a path with `,` and `:` worked and showed as `\054` in mountinfo.
  - `upperdir`/`workdir` strings are still backslash-unescaped even via fsconfig ([ovl_kern_path](https://github.com/torvalds/linux/blob/v6.18/fs/overlayfs/params.c#L384-L396)).
  - `FSCONFIG_SET_FD` for lowerdir+/datadir+/upperdir/workdir works since 6.13 and avoids all of this ([local] long path OK).
  - `override_creds` since 6.15 [docs].

---

## 4. Local measurements (btrfs, zstd:1, 12 cores, hot cache; hyperfine)

| Operation | Result |
|---|---|
| `unshare -Urm true` | 1.8 ± 0.4 ms (30 runs) |
| `unshare -Urm` + overlay mount (lower = 10k-file tree, userxattr) | 11.3 ± 3.0 ms (30 runs) |
| `cp -a --reflink=always` 10,003 files / 232 MB | 492.6 ± 23.6 ms |
| `cp -a --reflink=never` same tree | 844.5 ± 19.4 ms |
| reflink 10,000 × 4 KiB only | 495.2 ± 25.5 ms (≈50 µs/file) |
| reflink 3 × 64 MiB only | 1.4 ± 0.2 ms (`btrfs fi du`: 0 B exclusive) |
| full copy 3 × 64 MiB | 230.3 ± 18.4 ms |
| overlay copy-up of 64 MiB lower file, upper on **same btrfs** | 1.4 ms (open O_RDWR), 2.8 ms (utimes); upper 0 B exclusive |
| same copy-up, upper on **tmpfs** | 44.5 ms, 65 MB used |

Cleanup gotcha [local]: an overlay workdir keeps `work/` at mode 000 plus a shared 0:0 whiteout (`work/#52`), so a
plain `rm -rf` fails. `chmod -R u+rwx` first; the files are owned by the user.

---

## 5. Mount propagation, locked mounts, mountpoints

- A new mount ns owned by a new userns is "less privileged": **shared mounts are reduced to slave** and inherited
  mounts are **locked** ([mount_namespaces(7)](https://man7.org/linux/man-pages/man7/mount_namespaces.7.html) points 1-3).
  [local] with `--propagation unchanged`, the host's `/: shared:1` showed up as `master:1`.
- Locked mounts cannot be unmounted individually: `umount2("/home")` → EINVAL, also with MNT_DETACH [local]. Mounts made
  inside the ns unmount fine.
- Locked flags (RDONLY, NOSUID, NOEXEC, atime) cannot be relaxed (man point 5); adding restrictions works ([local]
  `mount -o remount,bind,ro /home` OK).
- Recommended: `mount_change("/", MS_REC|MS_PRIVATE)` right after unshare. This is what unshare(1) does by default,
  it keeps host mount events out mid-build, and it is required before any `pivot_root`.
- `mkdir /work` → **EACCES** even as ns-root [local]. You cannot create mountpoints in root-owned dirs.
- Bind-mounting a user dir onto another user dir works [local].
- A tmpfs over an existing root-owned dir (e.g. `/mnt`) works and lets you `mkdir` mountpoints inside it [local].
  On WSL2 this would hide `/mnt/c`.
- If the host `rmdir`s or renames a directory that is a mountpoint only inside the ns, **the mount vanishes from the ns**
  (man point 6). A host-side `rm -rf` of a canonical path only sees the empty underlying dir.
- `unshare --map-current-user -m sh -c 'mount …'` fails ("must be superuser"): exec as a non-zero uid drops the caps.
  It works with `--keep-caps` [local]. With uid mapped to itself, the exec'd program has **CapEff=0** [local, vm], which is desirable.

---

## 6. Joining an existing sandbox

- Rules ([setns(2)](https://man7.org/linux/man-pages/man2/setns.2.html)):
  - Joining a userns needs CAP_SYS_ADMIN in it. A same-euid process in the parent ns is the owner and qualifies.
  - The caller must be single-threaded, must not share CLONE_FS, and cannot re-enter its own userns (EINVAL; hit by accident [local]).
  - Joining the mount ns needs CAP_SYS_ADMIN + CAP_SYS_CHROOT in the caller's own userns, so join the user ns first, or pass both flags with a pidfd (atomic, since 5.8).
- Reading `/proc/PID/ns/*` is a `PTRACE_MODE_READ_FSCREDS` check ([namespaces(7)](https://man7.org/linux/man-pages/man7/namespaces.7.html)).
  Yama only hooks `PTRACE_MODE_ATTACH` ([yama_lsm.c L355](https://github.com/torvalds/linux/blob/v6.18/security/yama/yama_lsm.c#L349-L356)), so ptrace_scope 1/2/3 do not block it.
  Scope 1 verified [local]; 2/3 by source only.
- [local] holder `unshare -Urm … sleep`, joined from an unrelated shell:
  - `nsenter --preserve-credentials -U -m -t PID` works.
  - Without `--preserve-credentials`: `setgroups failed: EPERM` (setgroups is denied in the ns).
  - `-m` alone: EPERM.
- [local] Rust: `pidfd_open(holder)` in the parent, then `move_into_thread_name_spaces(pidfd, USER|MOUNT)` inside
  `Command::pre_exec` → the child sees the sandbox mounts, uid 1000, CapEff 0.
- **cwd after join is `/`**, and std runs `chdir(current_dir)` *before* `pre_exec` closures
  ([unix.rs L343 vs L388](https://github.com/rust-lang/rust/blob/1.99.0/library/std/src/sys/process/unix/unix.rs#L343-L389)), so chdir inside the closure.
- [vm] On Ubuntu, a joiner **without** the AppArmor profile (label `unconfined`) can join a sandbox created under the profile.
- Lifetime: a namespace lives while a process, an open `/proc/PID/ns/*` fd, or a bind mount of one exists (namespaces(7)).
  Unprivileged code cannot bind-mount ns files on the host (mounting there needs CAP_SYS_ADMIN over the host mount ns), so use a holder process or a held fd.

---

## 7. Reflink support matrix

| FS | FICLONE | Evidence |
|---|---|---|
| btrfs | yes | [local] |
| XFS | yes, `reflink=1` default since xfsprogs 5.1.0 (Jul 2019), needs `crc=1`; zoned XFS conflicts (6.15) | [src mkfs.xfs.8](https://git.kernel.org/pub/scm/fs/xfs/xfsprogs-dev.git/tree/man/man8/mkfs.xfs.8.in), [CHANGES](https://git.kernel.org/pub/scm/fs/xfs/xfsprogs-dev.git/tree/doc/CHANGES) |
| OpenZFS | 2.2.0: always on; 2.2.2–2.2.x: `zfs_bclone_enabled=0` default; 2.3.0+ (incl. 2.4.4): default 1; disabled → EOPNOTSUPP | [src 2.2.2](https://github.com/openzfs/zfs/blob/zfs-2.2.2/module/os/linux/zfs/zpl_file_range.c#L34), [2.4.4](https://github.com/openzfs/zfs/blob/zfs-2.4.4/module/zfs/zfs_vnops.c#L69); pool feature requirement [unverified] |
| bcachefs | removed from mainline in 6.18 (present in v6.17), out-of-tree only | [src](https://github.com/torvalds/linux/tree/v6.17/fs/bcachefs) |
| ext4 | no (`remap_file_range` absent) → EOPNOTSUPP | [src file.c](https://github.com/torvalds/linux/blob/v6.18/fs/ext4/file.c), [vm] `cp --reflink=always` fails |
| tmpfs | no → EOPNOTSUPP | [local] |
| across filesystems | EXDEV | [local] |
| overlayfs | yes if both real files are on the same reflink fs | [local] FICLONE inside a merged dir OK |

Probe: create two temp files in the destination dir and `ioctl(dst, FICLONE, src)`. Empty files are enough: btrfs → OK,
tmpfs → EOPNOTSUPP [local]. **FICLONE does not preserve mtime** (dst mtime = now, [local] in C and Rust), so follow
it with `utimensat`/`futimens`. `cp -a` does that (kept 1700000000 [local]).

---

## 8. Rust: rustix 1.1.5 (latest, 2026-09-16, MSRV 1.65; [crates.io](https://crates.io/crates/rustix), [docs.rs](https://docs.rs/rustix/1.1.5/rustix/))

Signatures read from the 1.1.5 crate source and exercised in `$SCRATCH/research-ns/rxtest` [local + vm].

| Need | API | Feature | Safety |
|---|---|---|---|
| unshare | `thread::unshare_unsafe(UnshareFlags::NEWUSER \| NEWNS)` | `thread` | **unsafe**: `FILES` can split fd tables between threads. `thread::unshare` is deprecated since 1.1.0 |
| setns via /proc fd | `thread::move_into_link_name_space(fd, Some(LinkNameSpaceType::User))` | `thread` | safe fn; the single-thread rule is not enforced |
| setns via pidfd | `process::pidfd_open(pid, PidfdFlags::empty())` + `thread::move_into_thread_name_spaces(pidfd, ThreadNameSpaceType::USER \| MOUNT)` | `process`, `thread` | safe fns |
| propagation | `mount::mount_change(c"/", MountPropagationFlags::REC \| PRIVATE)` | `mount` | safe |
| bind | `mount::mount_bind(src, dst)`; legacy `mount::mount(src, dst, fstype, MountFlags, Option<&CStr>)` | `mount` | safe |
| new mount API | `fsopen(c"overlay", FSOPEN_CLOEXEC)`, `fsconfig_set_string`, `fsconfig_set_flag(c"userxattr")`, `fsconfig_set_fd`, `fsconfig_create`, `fsmount(fd, FSMOUNT_CLOEXEC, MountAttrFlags::empty())`, `move_mount(&mnt, c"", fs::CWD, dst, MOVE_MOUNT_F_EMPTY_PATH)`, `open_tree` | `mount` | safe. No `mount_setattr` in 1.1.5 |
| unmount | `mount::unmount(path, UnmountFlags::DETACH)` | `mount` | safe |
| reflink | `fs::ioctl_ficlone(dst, src)` | `fs` | safe |
| lock | `fs::flock(fd, FlockOperation::NonBlockingLockExclusive)` | `fs` | safe |
| mtimes | `fs::utimensat(dirfd, path, &Timestamps, AtFlags)`, `fs::futimens` | `fs` | safe |
| fs type | `fs::statfs(path)?.f_type`, `fs::fstatfs` (btrfs 0x9123683e, tmpfs 0x01021994 [local]); only PROC/NFS magic constants are exported | `fs` | safe |

The only `unsafe` here is `unshare_unsafe` plus std's `CommandExt::pre_exec`; the closure must be async-signal-safe.
Pass pre-built `&CStr` arguments: rustix copies `path::Arg` values of 256 bytes or more to the heap, so a long `&str`
would allocate after fork. Read `getuid()`/`getgid()` **before** `unshare`: afterwards they return the overflow uid 65534,
and writing that into uid_map → EPERM [local].

---

## Not verified (treat as open)

- Docker on an AppArmor host: whether the `docker-default` profile also blocks the overlay mount with `seccomp=unconfined`.
- The runtime value of the sysctl on GitHub-hosted runners. Inferred from Ubuntu defaults plus flatpak's workaround.
- Ubuntu 24.10's kernel default for `restrict_unprivileged_unconfined`; 25.04+ behavior was not run in a VM.
- SELinux `user_namespace { create }` mediation on Fedora/RHEL.
- XFS or ZFS as overlay upper in a userns; the ZFS pool feature requirement.
- WSL2 runtime behavior.
- The stacked profile under heavier workloads: proc-macros were not exercised; sccache daemons, debuggers and linker plugins were not tried.

---

## Design implications for Ferry

1. **`ferry doctor` / mode selection by probe.** Fork a child. In it: `unshare(USER|NS)`, read `/proc/self/attr/current`,
   write setgroups → uid_map → gid_map, `mount_change`, then mount an overlay on a scratch dir. Report the failing step and
   errno, mapped to a fix:
   - ENOSPC → `user.max_user_namespaces=0`.
   - EPERM at unshare → seccomp/container or Debian `unprivileged_userns_clone=0`.
   - label `unprivileged_userns`, or EPERM/EACCES at setgroups/uid_map → Ubuntu AppArmor; offer `ferry setup apparmor`.
   - EINVAL from overlay → kernel < 5.11, or upper on overlayfs/container rootfs.
2. **Two tiers.**
   - (a) **Namespace mode**: userns + mntns + overlay + canonical paths, giving incremental reuse.
   - (b) **Plain mode**: per-agent reflink clone (FICLONE, falling back to copy) + stamped mtimes at the real path. Crate-level reuse only; never a shared target dir.
   - Pick (b) automatically when the probe fails and say why.
3. **Overlay recipe**:
   - `userxattr` always; never request metacopy/redirect_dir/index.
   - Use `fsopen`/`fsconfig`/`fsmount`/`move_mount` with `lowerdir+` (≥ 6.8) and `SET_FD` (≥ 6.13), with legacy `mount(2)` + escaping (`\`, `,`, `:`) for 5.11–6.7.
   - Refuse `\` in upper/workdir paths. Keep string values under 256 bytes, or use fds.
4. **Put each agent's upper/work on the same filesystem as the base target** (compare `st_dev`; check `statfs` f_type
   for btrfs/XFS). Copy-up then becomes a reflink, which removes most of the upper-layer cost. On ext4, warn that
   first-edit copy-ups (relinked binaries and rustc's hard-linked incremental files) copy full data.
5. **Exclusivity is Ferry's job.**
   - One mount namespace per agent, held by a holder process that keeps an `flock` on `agents/<name>/lock`.
   - Every further `ferry run` joins via `pidfd_open` + `setns(USER|MOUNT)` in `pre_exec`, then `chdir`.
   - Never mount a second overlay on the same upper: the kernel only warns.
   - Use the lock, not the kernel's in-use flag, as the source of truth.
6. **Thread model**: if Ferry is multithreaded (tokio, rayon), do `unshare`/`setns` only in a forked child (`pre_exec`) or
   a dedicated single-threaded helper. Otherwise you get EINVAL.
7. **Identity**:
   - Map uid→uid and gid→gid (setgroups "deny"), not root: agents keep their normal uid, git ownership checks pass, and CapEff is 0 after exec.
   - Read uid/gid before unshare. Do every mount in Ferry's own process before exec.
8. **Canonical paths**:
   - Use a user-owned, stable, per-repo path, e.g. `$FERRY_ROOT/mnt/{src,target}` under `$XDG_CACHE_HOME`, created once and never deleted while sandboxes live (man point 6).
   - Build the base at the same canonical path.
   - Don't shadow the main repo: worktrees' `.git` points at `<main>/.git/worktrees/<name>` (git worked fine at the canonical path [vm]).
   - The prototype's `mkdir /work` cannot work unprivileged.
9. **volatile**: worth it (no fsync on copy-up). Record `/proc/sys/kernel/random/boot_id` at mount. On remount, delete
   `work/work/incompat/volatile` only if the boot_id is unchanged; otherwise discard upper/work.
10. **`ferry rm`/`gc`**: `chmod -R u+rwx` workdirs before removal. Only remove canonical mountpoint dirs when no holder lock is held.
11. **Ubuntu**:
    - Ship `contrib/apparmor/ferry` and `ferry setup apparmor`, which resolves the real binary path, refuses user-writable locations, writes the profile and runs `apparmor_parser -r`.
    - Default to the stacked (bwrap-style) profile. The plain `flags=(unconfined)` one exempts every agent process from the restriction.
    - Document the sysctl as a last resort.
12. **CI (GitHub Actions ubuntu-24.04/latest)**:
    - Most jobs: `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`, needed if libtest binaries call unshare directly.
    - One job: install the binary at `/usr/local/bin/ferry`, then `sudo install -m644 contrib/apparmor/ferry /etc/apparmor.d/ferry && sudo apparmor_parser -r /etc/apparmor.d/ferry`, then run the e2e suite with the sysctl left at 1.
13. **Containers**: detect seccomp EPERM and fall back to plain mode. Document `--security-opt seccomp=unconfined`, plus
    the requirement that Ferry's root (uppers) lives on a volume or tmpfs, not the container's overlay rootfs.

## Open questions

- Holder lifetime policy: idle timeout vs explicit `ferry stop`. What happens to running agent processes when the holder dies?
  The ns survives while any member process lives.
- Should plain mode prefer reflink clone (~50 µs/file) or overlay-less hardlink farms? Hardlinks share mtimes and inodes,
  which is risky with Cargo's in-place writes.
- Is `-Zchecksum-freshness` close enough to stabilisation to drop mtime stamping, and does it change the overlay
  copy-up pattern (fewer `utimes` copy-ups)?
- Can rustc be made to copy-on-write incremental session dirs (or use reflink in `link_or_copy`) to cut ext4 copy-up cost?
- Does the stacked AppArmor profile break agent tooling that needs capabilities (`perf`, `gdb` attach, nested `bwrap`, rootless podman)?
- Ubuntu ≥ 25.04 VM run of the same probes (`restrict_unprivileged_unconfined=1`), and Docker on an Ubuntu host.
