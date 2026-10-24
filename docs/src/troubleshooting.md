# Troubleshooting

Start with `ferry doctor`. Every warning it prints comes with a `fix:` line,
and `ferry --json doctor` shows the raw probe results and relevant sysctls.

## User namespaces

On Linux, the namespace sandbox needs unprivileged user and mount namespaces.
A quick manual check (it should print nothing and exit 0):

```sh
unshare --user --map-root-user --mount true
```

When they do not work, Ferry builds at host paths (dependency reuse only) and
`ferry base` warns:

```text
warning: namespace sandbox unavailable (...); building without canonical paths, so an agent's first edit rebuilds without incremental reuse. Run `ferry doctor` for fixes.
```

With `sandbox = "namespace"` it fails instead:

```text
error: namespace sandboxes are not supported on this platform: ...
  hint: run `ferry doctor` for details, or set FERRY_SANDBOX=off to build without the canonical-path sandbox
```

The common causes, in the order `ferry doctor` checks them:

| Cause | Typical systems | Fix |
|---|---|---|
| `kernel.apparmor_restrict_unprivileged_userns = 1` | Ubuntu 24.04 and later | install the AppArmor profile: see [below](#apparmor-on-ubuntu-2404-and-later) |
| `kernel.unprivileged_userns_clone = 0` | Debian kernels with the default changed | `sudo sysctl kernel.unprivileged_userns_clone=1` |
| `user.max_user_namespaces = 0` | RHEL hardened to the DISA STIG; NixOS with `security.allowUserNamespaces = false` | `sudo sysctl user.max_user_namespaces=15000` (on NixOS, set `security.allowUserNamespaces = true`) |
| a seccomp filter | Docker's default profile; agent sandboxes | see [Containers](#containers) and [Agent sandboxes](#agent-sandboxes) |

`sysctl` changes made this way last until the next reboot. To keep one, put
it in a file under `/etc/sysctl.d/`. Check your organisation's policy first:
these settings exist to reduce the kernel attack surface.

If you cannot or do not want to change the system, set `sandbox = "off"` (or
`FERRY_SANDBOX=off`). Everything still works, with dependency reuse: the
workspace's own crates rebuild once after spawn.

### Containers

Docker's default seccomp profile blocks `unshare` and `mount` for processes
without `CAP_SYS_ADMIN`, so inside Docker Ferry falls back to dependency
reuse. Options:

- Accept the fallback. Nothing else is needed.
- Run the container with a seccomp profile that allows `unshare` with
  `CLONE_NEWUSER` (for example `--security-opt seccomp=unconfined`), and keep
  Ferry's store on a volume or `--tmpfs` mount rather than the container's
  overlay root filesystem, which overlayfs cannot use as an upper layer.
- Use rootless Podman, whose default seccomp profile allows these calls.

These container facts come from Ferry's research probes and the profiles'
sources; Ferry itself has not been tested in containers.

### Agent sandboxes

Agent tools that sandbox their commands may block user namespaces. Claude
Code 2.1.305 and later do by default; see
[Claude Code's sandbox](agents/claude-code.md#claude-codes-sandbox). Codex's
Linux sandbox does not block them (see [Codex](agents/codex.md)). Running the
whole agent with `ferry run <name> -- <agent>` always works, because the
namespace exists before the agent's sandbox starts.

## AppArmor on Ubuntu 24.04 and later

Ubuntu 24.04 turned on an AppArmor policy that lets unprivileged processes
create user namespaces but gives them no capabilities inside. Ferry's sandbox
then fails at the uid mapping or the first mount, and `ferry doctor` shows:

```text
  ! namespaces  ...
    fix: Ubuntu's AppArmor policy denies capabilities in unprivileged user namespaces. Install ferry to a root-owned path (e.g. /usr/local/bin/ferry) and load the profile it prints: ...
```

The fix is an AppArmor profile that allows the `ferry` binary, and only it,
to use user namespaces:

```sh
# 1. Install ferry to a root-owned path.
sudo install -m 755 "$(command -v ferry)" /usr/local/bin/ferry

# 2. Generate the profile for that path and load it.
/usr/local/bin/ferry doctor --apparmor-profile | sudo tee /etc/apparmor.d/ferry
sudo apparmor_parser -r /etc/apparmor.d/ferry

# 3. Check.
/usr/local/bin/ferry doctor
```

The profile persists across reboots. The same profile for
`/usr/local/bin/ferry` is in the repository as
[`contrib/apparmor/ferry`](https://github.com/patwrall/ferry/blob/main/contrib/apparmor/ferry).
Ferry's CI runs its sandbox tests on Ubuntu 24.04 with the restriction on and
this profile loaded.

**Why a root-owned path.** AppArmor attaches the profile by path. If the path
is writable by your user (for example `~/.cargo/bin/ferry`), any process
running as you could replace the binary and inherit the permission. `ferry
doctor --apparmor-profile` warns when the binary's path is not root-owned.

**Make sure that binary is the one that runs.** The profile covers only
`/usr/local/bin/ferry`. Remove other copies (or put `/usr/local/bin` first on
`PATH`), and re-run `ferry shim` (or any command that uses the shim) from the
installed binary, so the shim link points at it. After upgrading Ferry,
install the new binary to the same path again; the profile stays valid.

**What the profile allows.** It is the stacked form Ubuntu uses for its own
`bwrap-userns-restrict` profile: `ferry` itself may create the namespace and
mount, while everything it runs (cargo, build scripts, tests, your agent) runs
under a second profile, `unpriv_ferry`, that denies capabilities. Your agent's
processes therefore cannot create privileged user namespaces themselves. A
simpler `flags=(unconfined)` profile would exempt the agent's whole process
tree from the restriction. The remaining trade-off is that any local process
that can run `/usr/local/bin/ferry` can use it to create user namespaces; see
[`SECURITY.md`](https://github.com/patwrall/ferry/blob/main/SECURITY.md).

**Last resort: turn the restriction off.**

```sh
sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0   # until reboot
```

This re-opens unprivileged user namespaces for every process on the machine,
including browsers and anything an attacker manages to run. That is the
kernel attack surface the restriction exists to close: AppArmor's
documentation lists exploits such as CVE-2022-0185 and CVE-2024-1086 that
were reachable through user namespaces. Prefer the profile. If neither is
acceptable, leave the restriction on and use `sandbox = "off"`.

## Filesystems

**overlayfs fails.** The overlay backend needs Linux 5.11 or newer and a store
filesystem that supports user extended attributes: ext4, btrfs and XFS do,
tmpfs only from Linux 6.6. Overlayfs cannot use another overlayfs as its
upper layer, which matters inside containers. Without overlay, `auto` uses
reflink or copy instead.

**Slow or flaky builds on a network filesystem.** `ferry doctor` warns when
the store is on NFS, CIFS or FUSE: builds are slow there and mtimes
unreliable. Point `FERRY_HOME` at a local disk.

**No reflinks.** Copy-on-write clones work on btrfs, XFS (created with
`reflink=1`, the default since xfsprogs 5.1), OpenZFS 2.2.0 and 2.3.0 or newer
(2.2.2 to 2.2.x have them off by default), and APFS. Not on ext4 or tmpfs, and
never across filesystems. Without them, agents use overlay (Linux, with the
namespace sandbox) or full copies.

**`path ... cannot be used in an overlayfs mount`.** On Linux kernels before
6.13, Ferry passes overlay paths as mount options, which cannot express `,`,
`:`, `\` or newlines. Set `FERRY_HOME` to a path without them.

## Disk usage

`ferry list` shows each base's size and each agent's `DISK`: the build state
it holds beyond the base.

- **Overlay agents** copy a file into their upper layer the first time they
  write to it. Relinked binaries and rustc's incremental session directories
  account for most of it. On ext4 every copy-up is a full copy: in the
  prototype on ripgrep (ext4) each agent's upper layer was about 190 MB over a
  536 MB base. The upper layer and the base are both in the store, so on
  btrfs and XFS a copy-up is a reflink that shares the data.
- **Reflink agents** share all data with the base until they rewrite a file.
- **Copy agents** cost a full copy of the target dir each.
- **Bases** stay until no agent uses them and they are not current. Every
  `ferry base` for a new commit creates a new base seeded from the previous
  one (a reflink clone where possible, else a full copy); the old one is
  deleted by `ferry gc` once no agent uses it.

To reclaim space: `ferry rm` agents you are done with, then `ferry gc`
(`--dry-run` first to see what it would delete). Agents forked from an old
base keep that base alive.

## Cargo configuration pitfalls

- **`build.target-dir` and `build.build-dir` in `.cargo/config.toml`** are
  overridden: Ferry sets `CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` for
  every build it runs, and Cargo's environment variables beat config files.
  Both are set because a `build-dir` template such as
  `{workspace-path-hash}` resolves to the same directory for every agent at
  the canonical path.
- **A `.cargo/config.toml` above the repository, or an untracked one in it,
  does not apply to Ferry's builds**, which run under the store where Cargo
  never finds it. `ferry base` and `ferry doctor` name such files. Move their
  settings into the repository's tracked `.cargo/config.toml` or into
  `$CARGO_HOME/config.toml`.
- **`--target-dir`, `--config build.target-dir=...` and
  `--config build.build-dir=...` on the command line beat the environment.**
  A build run that way leaves the agent's fork and can share state with other
  agents. Do not use them in agents or warm commands.
- **Differences between the base and the agent rebuild everything.** Cargo
  hashes `RUSTFLAGS`, `RUSTC_WORKSPACE_WRAPPER`, features and profile settings
  into its fingerprints. If agents build with different values than the warm
  commands, nothing is reused.
- **`CARGO_INCREMENTAL=0`** (set by some CI images and agent tools) switches
  off incremental compilation, and with it Ferry's incremental reuse. Unset it.
- **Build caches as `RUSTC_WRAPPER`.** kache strips Cargo's incremental
  argument, so a base built through it has no incremental state; sccache
  refuses incremental units. Neither changes Cargo's fingerprints, so
  crate-level reuse still works.

## Builds that rebuild more than expected

- **`warning: N of M tracked files differ from base ...`** at spawn: the agent
  starts from a different commit than its base (`--from`), so crates using the
  changed files rebuild on the first build. Run `ferry base --rev <commit>` to
  have a base at that commit, or keep `ferry base --watch` running so the base
  follows `HEAD`. The warning says when a base for the agent's commit is
  already being built.
- **The workspace's own crates rebuild after a spawn without the namespace
  sandbox.** The agent builds at other absolute paths than its base, and any
  local crate may have captured the base's (a build script writing
  `CARGO_MANIFEST_DIR` into generated code, a proc macro reading it), so Ferry
  makes them all rebuild once. Dependencies stay reused. Fix the namespace
  sandbox (`ferry doctor`) to avoid it.
- **A refresh is a cold build after a manifest change.** `ferry base` seeds
  across a change to `Cargo.toml`s, `Cargo.lock`s, Cargo configs, toolchain
  files or `RUSTFLAGS`-style variables only from a base whose warm build it
  observed (`"observed": true` in `ferry --json list`), because it must then
  delete the units the new build does not use. Warm commands that run
  `cargo nextest`, an alias like `cargo xtask`, or pass
  `--message-format=short` are not observed; see
  [Refreshing a base](configuration.md#refreshing-a-base). `[base] prune =
  false` turns this off on purpose.
- **`warning: rebuilt the base from scratch: could not tell which of the
  seed's units the warm commands used (...)`** from `ferry base`: the seed's
  warm build was observed but this one was not, or something in the target
  dir did not match what Cargo reported. The reason in parentheses says
  which. The base is correct, just built cold; a cargo call that bypasses the
  shim and rebuilds something is the usual cause.
- **`warning: the warm commands wrote N file(s) into the source tree ...`**
  from `ferry base`: a build script writes outside `OUT_DIR`. Agents do not get
  those files, so whatever reads them may rebuild or behave differently.
- **Every crate rebuilds after a toolchain update.** The base's fingerprint
  includes the toolchain; run `ferry base` again.

## Builds that rebuild less than they should

Cargo's default freshness check compares modification times. A file whose new
content arrives with an old mtime (copied with `cp -p` or `rsync -t`,
extracted from an archive, or set with `touch -d`) can look unchanged to
Cargo, with or without Ferry. `touch` the file to force a rebuild. On nightly
toolchains, Ferry's `auto` freshness mode uses Cargo's checksum freshness,
which compares rustc's inputs by content; build-script inputs stay mtime-based.

Ferry stamps files only once, when the agent is created, and only files that
are byte-for-byte identical to the base; it never re-stamps a live worktree.

## Errors and odd states

- **An untracked `Cargo.lock` in a new worktree.** If the repository does not
  track `Cargo.lock`, Ferry copies the base's into each new worktree so agents
  resolve the same dependency versions. `ferry rm`, the Claude Code
  `WorktreeRemove` hook and `ferry gc` recognise that unchanged copy and do not
  treat it as the agent's work; once the agent changes it (say, by adding a
  dependency), it counts as uncommitted.
- **`agent ... was not fully created`**, or `incomplete` in `ferry list`:
  spawning was interrupted. Remove it with `ferry rm <name>`.
- **`base ... is missing or incomplete`**: run `ferry base` to rebuild it,
  then `ferry gc`.
- **`the agent's overlay is still mounted by processes that cannot be joined (pids: ...)`**:
  processes started inside the agent's sandbox created their own namespaces
  (`unshare`, `bwrap`, a container) and still have the agent's overlay
  mounted after the agent's own sessions ended. A second mount of the same
  overlay would corrupt it, so Ferry waits up to 10 seconds and then refuses.
  Wait for those processes or stop them, then retry.
- **`the agent's mount lock is still held by ...`**: a session that is
  exiting still holds the agent's overlay while the kernel unmounts it,
  which can take seconds under heavy disk writes. Retry; if it persists, the
  named processes (or ones in another pid namespace) are still using it.
- **Paths printed by cargo do not exist.** Inside the namespace sandbox, cargo
  prints canonical paths such as `<store>/mnt/target/debug/deps/app-...`.
  They exist only inside the sandbox; on the host those directories are empty.
  Run binaries and tests through `ferry run` or `ferry shell`.
- **``ferry (cargo shim): no real `cargo` found on PATH after the shim``**: the
  shim's directory is on `PATH`, but no other `cargo` is. Install Rust with
  rustup, or fix the `PATH` order.
- **`refusing to continue: branch ... already exists`**: an earlier agent's
  branch was kept because it has commits. Pass `--branch` to use another name,
  or merge or delete the old branch.
