# Troubleshooting

Start with `ferry doctor`. Every warning it prints comes with a `fix:` line,
and `ferry --json doctor` shows the raw probe results and relevant sysctls.

## User namespaces

On Linux, the namespace sandbox needs unprivileged user and mount namespaces.
A quick manual check (it should print nothing and exit 0):

```sh
unshare --user --map-root-user --mount true
```

When they do not work, Ferry builds at host paths (crate-level reuse only) and
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
| `kernel.apparmor_restrict_unprivileged_userns = 1` | Ubuntu 24.04 and later | `ferry setup apparmor`: see [below](#apparmor-on-ubuntu-2404-and-later) |
| `kernel.unprivileged_userns_clone = 0` | Debian kernels with the default changed | `sudo sysctl kernel.unprivileged_userns_clone=1` |
| `user.max_user_namespaces = 0` | RHEL hardened to the DISA STIG; NixOS with `security.allowUserNamespaces = false` | `sudo sysctl user.max_user_namespaces=15000` (on NixOS, set `security.allowUserNamespaces = true`) |
| a seccomp filter | Docker's default profile; agent sandboxes | see [Containers](#containers) and [Agent sandboxes](#agent-sandboxes) |

`sysctl` changes made this way last until the next reboot. To keep one, put
it in a file under `/etc/sysctl.d/`. Check your organisation's policy first:
these settings exist to reduce the kernel attack surface.

If you cannot or do not want to change the system, set `sandbox = "off"` (or
`FERRY_SANDBOX=off`). Everything still works, with crate-level reuse: only
crates that captured the base's absolute paths rebuild after spawn, and the
first edit gets no incremental reuse.

### Containers

Inside a container, Ferry falls back to crate-level reuse unless the container
allows the namespace sandbox. Accepting the fallback needs nothing else.
Otherwise up to four layers are in the way:

1. **Seccomp.** Docker's default profile allows `unshare`, `mount`, `setns`,
   `fsopen`, `fsconfig`, `fsmount` and `move_mount` only to containers with
   `CAP_SYS_ADMIN`, and fails them with `EPERM` otherwise. Rootless Podman's
   default profile allows them.
2. **AppArmor, on AppArmor hosts (Ubuntu, Debian, SUSE).** Docker's
   `docker-default` and rootful Podman's `containers-default` profiles contain
   `deny mount`, so Ferry's bind and overlay mounts fail even inside its own
   user namespace. Rootless Podman applies no AppArmor profile.
3. **Ubuntu 24.04 and later hosts.** With `apparmor=unconfined`, container
   processes are AppArmor's `unconfined` profile, which the host's
   restriction moves to `unprivileged_userns`, so nothing inside the container
   fixes this. Either lift the restriction on the host, or load a container
   AppArmor profile without `deny mount` on the host and name it with
   `--security-opt apparmor=<name>`. `ferry setup apparmor` refuses to run
   inside a container, because only the host can load AppArmor policy.
4. **The store's filesystem.** The container's root filesystem is overlayfs,
   which overlayfs cannot use as its upper layer (`ferry doctor` warns when
   the store is on overlayfs). Put `FERRY_HOME` on a volume or bind mount
   (ext4, XFS or btrfs), or on a tmpfs on Linux 6.6 or newer that allows exec.
   Without that, `auto` uses reflink or copy, and the sandbox itself still
   works. `FERRY_HOME` must be owned by the container user and not group or
   world writable, so a fresh volume needs a `chown`; worktrees then default
   to `$FERRY_HOME/worktrees`, which survives container rebuilds.

For seccomp, either use `--security-opt seccomp=unconfined`, which removes
all syscall filtering, or your Docker version's default profile with two
rules added: `unshare` only with exactly the flags Ferry passes
(`CLONE_NEWUSER | CLONE_NEWNS` = 268566528), and the mount calls:

```sh
jq '.syscalls += [
  {"names":["unshare"],"action":"SCMP_ACT_ALLOW","args":[{"index":0,"value":268566528,"op":"SCMP_CMP_EQ"}]},
  {"names":["mount","setns","fsopen","fsconfig","fsmount","move_mount"],"action":"SCMP_ACT_ALLOW"}
]' default.json > .devcontainer/ferry-seccomp.json
```

`setns` joins a running overlay sandbox; the `fs*` calls are optional, as
Ferry falls back to `mount(2)`. The kernel still requires `CAP_SYS_ADMIN`
over the target namespace, so container root gains no mount power outside
user namespaces. Do not use `--privileged` or `--cap-add SYS_ADMIN`: they
grant real `CAP_SYS_ADMIN`, far more than Ferry needs.

A devcontainer that does all of this (on a Debian host, or any AppArmor host
without Ubuntu's restriction, add `"apparmor=unconfined"` to `securityOpt`):

```jsonc
{
  "image": "mcr.microsoft.com/devcontainers/rust:1",
  "securityOpt": ["seccomp=unconfined"], // or "seccomp=${localWorkspaceFolder}/.devcontainer/ferry-seccomp.json"
  "mounts": [{ "type": "volume", "source": "ferry-${devcontainerId}", "target": "/ferry" }],
  "containerEnv": { "FERRY_HOME": "/ferry" },
  "postCreateCommand": "sudo chown \"$(id -u):$(id -g)\" /ferry && chmod 700 /ferry"
}
```

Docker Desktop runs containers in a Linux VM without the host's AppArmor, so
seccomp should be the only layer there. On SELinux hosts, nested container
tooling commonly needs `--security-opt label=disable`.

These container facts come from the profiles' and the kernel's sources and
from standalone probes; Ferry itself has not been tested in containers.

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
  ! namespaces  setgroups failed: Permission denied ...
    fix: Ubuntu's AppArmor policy denies capabilities in unprivileged user namespaces: run `ferry setup apparmor`, ...
```

The fix is an AppArmor profile that allows the `ferry` binary, and only it,
to use user namespaces. `ferry setup apparmor` installs it:

```sh
ferry setup apparmor            # shows every step, then asks
ferry setup apparmor --dry-run  # only shows them
```

It prints why the setup is needed, each step, the profile, the root script
and the exact `sudo` command, and asks before running that one command. As
root, it copies the running ferry to `/usr/local/bin/ferry` (owned by root),
writes `/etc/apparmor.d/usr.local.bin.ferry`, checks that it parses, removes
older Ferry profiles such as `/etc/apparmor.d/ferry`, and loads it. Then, as
you, it checks that `/usr/local/bin/ferry` can create a user namespace and
points the cargo shim at it. Run `ferry base` again in each repository
afterwards: agents spawned before keep building at host paths until they are
spawned again. See [`ferry setup apparmor`](cli.md#ferry-setup-apparmor) for
its options and the cases where it refuses. Ferry's CI sets up its Ubuntu
24.04 AppArmor job this way, with the restriction on.

The profile persists across reboots. **After upgrading Ferry,** run the
upgraded copy: after `cargo install`, that is `~/.cargo/bin/ferry setup
apparmor`. It installs the new binary and leaves the profile alone. A bare
`ferry` may still be the old `/usr/local/bin/ferry`, which reports that it is
already set up and says when `~/.cargo/bin/ferry` is a different build. **To
undo it,** run `ferry setup apparmor --uninstall`, which unloads
and removes Ferry's profile files and leaves `/usr/local/bin/ferry` for you to
remove.

To do it by hand instead (remove an older `/etc/apparmor.d/ferry` first, so
that only one file defines the profile):

```sh
# 1. Install ferry to a root-owned path.
sudo install -m 755 "$(command -v ferry)" /usr/local/bin/ferry

# 2. Generate the profile for that path and load it.
/usr/local/bin/ferry doctor --apparmor-profile | sudo tee /etc/apparmor.d/usr.local.bin.ferry
sudo apparmor_parser -r /etc/apparmor.d/usr.local.bin.ferry

# 3. Check.
/usr/local/bin/ferry doctor
```

The same profile for `/usr/local/bin/ferry` is in the repository as
[`contrib/apparmor/usr.local.bin.ferry`](https://github.com/patwrall/ferry/blob/main/contrib/apparmor/usr.local.bin.ferry).

**Why a root-owned path.** AppArmor attaches the profile by path. If the path
is writable by your user (for example `~/.cargo/bin/ferry`), any process
running as you could replace the binary and inherit the permission. `ferry
setup apparmor` therefore installs a copy, and `ferry doctor
--apparmor-profile` warns when the binary's path is not root-owned. If
`/usr/local/bin` is group-writable on your system, pass `--bin-dir
/opt/ferry/bin`.

**Make sure that binary is the one that runs.** The profile covers only
`/usr/local/bin/ferry`. Remove other copies, or put `/usr/local/bin` first on
`PATH`: the shim link follows whichever ferry ran last, and Claude Code's
hooks call `ferry` through `PATH`. `ferry setup apparmor` warns when the first
`ferry` on `PATH` is another copy, and `ferry doctor` says when a Ferry
profile covers a different binary than the one you ran.

**What the profile allows.** It is the stacked form Ubuntu uses for its own
`bwrap-userns-restrict` profile: `ferry` itself may create the namespace and
mount, while everything it runs (cargo, build scripts, tests, your agent) runs
under a second profile, `unpriv_ferry`, that denies capabilities. Your agent's
processes therefore cannot create privileged user namespaces themselves. A
simpler `flags=(unconfined)` profile would exempt the agent's whole process
tree from the restriction. One exception: ferry re-running its own binary (the
namespace probe does) stays under `ferry` (an `ix` rule for its exact path),
because a capability-less probe would report that namespaces do not work. The
remaining trade-off is that any local process that can run
`/usr/local/bin/ferry` can use it to create user namespaces. On Ubuntu 24.04
and 24.10, where `kernel.apparmor_restrict_unprivileged_unconfined` is `0`,
an unconfined process can also run any program under the `ferry` profile with
`aa-exec -p ferry`; that holds for every user-namespace profile Ubuntu ships,
and 25.04 and later set the sysctl to `1`. See
[`SECURITY.md`](https://github.com/patwrall/ferry/blob/main/SECURITY.md).

**Tools that need their own namespaces break under `ferry run`.** Because of
that stacking, a sandbox tool such as `bwrap` (used by Claude Code's sandbox)
or a test that calls `unshare` gets no capabilities inside its namespace and
fails. Ubuntu ships its own `bwrap` profile disabled for similar breakage.

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
upper layer, which matters inside containers: `ferry doctor` warns when the
store is on overlayfs, and the fix is a volume for `FERRY_HOME` (see
[Containers](#containers)). `auto` uses overlay only where copy-on-write
clones are unavailable; without either, it uses copy.

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

- **Reflink agents** share all data with the base until they rewrite a file.
  `du` counts shared data once per agent, so it reports about a full target
  dir for each; `btrfs filesystem du` or `compsize` show what is exclusive.
- **Overlay agents** (where clones are unavailable, such as ext4) copy a file
  into their upper layer the first time they write to it. Relinked binaries
  and rustc's incremental session directories account for most of it. On
  ext4 every copy-up is a full copy: in the prototype on ripgrep (ext4) each
  agent's upper layer was about 190 MB over a 536 MB base.
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
  commands, nothing is reused. `RUSTFLAGS=-Dwarnings` is such a difference.
- **`cargo clippy -- -D warnings` re-lints every workspace member** from
  scratch, because the base warmed clippy without lint flags (dependencies
  stay fresh). Use `CARGO_BUILD_WARNINGS=deny cargo clippy` instead
  (Cargo 1.97+), which turns warnings into errors without touching any
  fingerprint.
- **`CARGO_INCREMENTAL=0`** (set by some CI images and agent tools) switches
  off incremental compilation, and with it Ferry's incremental reuse. Unset it.
- **`CI` or `TF_BUILD` set** (some agent tools export `CI`). Cargo 1.99 and
  later then turn incremental compilation off, ahead of every profile
  setting, and since a unit's hash includes it, an agent would also rebuild
  every workspace crate of a base warmed without it. So when either variable
  is set, Ferry sets `CARGO_INCREMENTAL=1` for the cargo it runs in agents
  and warm commands, unless `CARGO_INCREMENTAL`, `CARGO_BUILD_INCREMENTAL` or
  `build.incremental` (in a Cargo config or `--config`) is set. `CI` and
  `TF_BUILD` reach build scripts, tests and `cargo run` unchanged. So does
  `CARGO_INCREMENTAL=1`: a cargo call they make on a workspace outside the
  agent builds incrementally too. This has costs. Release and bench builds
  become incremental, so they are bigger and slower. Their units match
  between a base and an agent only when both have `CI` or `TF_BUILD` set, or
  both lack them. `incremental = false` in a `[profile.*]` is overridden. A
  refresh across a change in whether `CI` or `TF_BUILD` is set, or in
  `CARGO_INCREMENTAL`, is seeded across unit inputs, so it is pruned. To opt
  out, set `CARGO_INCREMENTAL=0`, which turns incremental compilation off as
  plain Cargo does under `CI`.
- **Build caches as `RUSTC_WRAPPER`.** kache strips Cargo's incremental
  argument, so a base built through it has no incremental state; sccache
  refuses incremental units. Neither changes Cargo's fingerprints, so
  crate-level reuse still works.

## Builds that rebuild more than expected

- **`warning: N of M tracked files differ from base ...`** at spawn: the agent
  starts from a different commit than its base (`--from`), so crates using the
  changed files rebuild on the first build. Run `ferry base --rev <commit>` to
  have a base at that commit (the warning names it), or, for agents started
  at `HEAD`, keep `ferry base --watch` running so the base follows it. The
  warning says when a base for the agent's commit is already being built.
- **`warning: N build units captured the base's absolute paths ...`** at a
  spawn without the namespace sandbox. The agent builds at other absolute
  paths than its base, and those crates recorded the base's: library code
  reading `env!("CARGO_MANIFEST_DIR")`, a build script writing it into
  generated code or a `rustc-env` value, users of a proc macro that reads it,
  tests using `env!("CARGO_BIN_EXE_...")`. They rebuild once, with whatever
  depends on them; everything else stays reused. Fix the namespace sandbox
  (`ferry doctor`) to avoid it, or stop baking the path in (read it at run
  time, or make it relative).
- **A refresh is a cold build after a manifest change.** `ferry base` seeds
  across a change to `Cargo.toml`s, `Cargo.lock`s, Cargo configs, toolchain
  files or `RUSTFLAGS`-style variables only from a base whose warm build it
  observed (`"observed": true` in `ferry --json list`), because it must then
  delete the units the new build does not use. The same goes for a change to
  `CARGO_INCREMENTAL`, or to whether `CI` or `TF_BUILD` is set. Warm
  commands that run `cargo nextest`, an alias like `cargo xtask`, or pass
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
