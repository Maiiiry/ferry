# Security policy

## Supported versions

Ferry is pre-1.0. Fixes go into the latest release only.

| Version                 | Supported |
| ----------------------- | --------- |
| Latest 0.x release      | Yes       |
| Older releases          | No        |

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's
[private vulnerability reporting](https://github.com/patwrall/ferry/security/advisories/new).
Please do not open a public issue. If you cannot use GitHub, email
patrickwrall@gmail.com.

Include the Ferry version, the output of `ferry doctor --json`, and steps to
reproduce. You should get a response within a week. Once a fix is released,
the advisory is published with credit to the reporter unless you prefer
otherwise.

## Threat model

**Ferry is not a security sandbox.** It isolates build state, not processes.

- Agents, and every command they run through Ferry, run with your full user
  privileges: the same uid, the same files, the same network. Ferry gives each
  agent its own worktree and target directory so that agents do not corrupt
  each other's builds. It does not stop an agent from reading or writing
  anything else you can.
- On Linux, Ferry uses user and mount namespaces for **path remapping only**:
  the agent's worktree and target directory are mounted at fixed paths so that
  Cargo and rustc see the same absolute paths in every agent. Host paths stay
  reachable inside the namespace, and your uid and gid map to themselves (not
  to root).
- Do not use Ferry to contain code you do not trust. Use a VM, a container, or
  your agent tool's own sandbox for that; Ferry runs inside whatever sandbox
  you start it in.

### What Ferry does on your system

- **Namespaces and mounts (Linux).** Ferry creates unprivileged user and mount
  namespaces, makes mount propagation private, and creates bind mounts and
  overlayfs mounts inside them. These mounts exist only inside the namespace
  and disappear when its last process exits. Ferry never mounts anything in
  the host's mount namespace and never runs anything as root; `ferry doctor`
  only suggests `sudo` commands for you to review and run.
- **Build commands.** `ferry base` runs the warm-up commands from the
  repository's `ferry.toml` (or `FERRY_BASE_COMMAND`) with `sh -c`. Like a
  `Makefile` or a `build.rs`, a `ferry.toml` in a repository you clone can run
  arbitrary commands when you build it. The `cargo` shim runs the next real
  `cargo` on your `PATH`.
- **The shim directory.** The directory that goes first on agents' `PATH`
  (`ferry shim`) is `$FERRY_HOME/bin` or `bin` in the platform cache dir. It
  is deliberately not configurable from `ferry.toml`, so a repository cannot
  choose what agents run as `cargo`.
- **Git.** Ferry runs your `git`, so worktree creation runs your git hooks,
  filters and LFS configuration, exactly as `git worktree add` does.
- **Files.** Ferry writes only under its store (`$FERRY_HOME`, the `[store]`
  directory in `ferry.toml`, or by default `$XDG_CACHE_HOME/ferry` or
  `~/.cache/ferry` on Linux and `~/Library/Caches/ferry` on macOS) and its
  worktree directory (`FERRY_WORKTREES`, `[agent] worktree-dir`, or by default
  `$XDG_DATA_HOME/ferry/worktrees` or `~/.local/share/ferry/worktrees` on
  Linux and `~/Library/Application Support/ferry/worktrees` on macOS), plus
  any worktree path you pass with `--path`. It creates the store, the shim
  directory and the per-repository worktree directory readable only by you,
  and refuses to use an existing one that you do not own or that others can
  write to: whoever could write there could swap what a build or an agent
  runs. Build outputs in the store can contain anything your build embeds, the
  same as `target/`.

### The optional AppArmor profile (Ubuntu)

Ubuntu 23.10 and later block unprivileged user namespaces
(`kernel.apparmor_restrict_unprivileged_userns=1`). `ferry doctor
--apparmor-profile` prints a profile that allows the `ferry` binary at its
current path to create user namespaces; installing it requires `sudo`.

Installing it is a trade-off. Any local process that can execute that `ferry`
binary can then create user namespaces through it, and `ferry run` runs
arbitrary commands inside them. That re-exposes, for those processes, the kernel
attack surface that the Ubuntu restriction exists to reduce. It is the same
trade-off Ubuntu makes in the profiles it ships for `bwrap`, Chrome and
`podman`. The profile matches the binary by path, so if that path is writable
by your user (for example `~/.cargo/bin/ferry`), any process running as you can
replace the binary and inherit the permission; `ferry doctor
--apparmor-profile` warns when the binary or any directory above it is
writable by a non-root user, and refuses paths with characters AppArmor would
read as a pattern. Installing Ferry to a root-owned path such as
`/usr/local/bin` limits the grant to the real binary.

If you do not want this trade-off, leave the restriction on and set
`sandbox = "off"`: builds then run at host paths, dependencies are still
reused, and the workspace's own crates rebuild once per agent.

### What counts as a vulnerability

Please report privately, for example:

- Ferry creating, deleting or modifying files outside its store and the
  worktrees it manages (for example through agent names, branch names, paths
  or configuration values).
- Ferry running a command or mount the user did not configure, or passing
  untrusted input to a shell.
- Ferry giving a process more privilege than the user running it has, or
  anything the AppArmor profile allows beyond what is described above.
- Store files or directories created readable or writable by other users.
- Problems with release artifacts, installers or the Homebrew formula.

A wrong build (for example, an agent getting another agent's code, or a stale
artifact counted as fresh) is a correctness bug; please open a regular issue,
where it will be treated as high priority.
