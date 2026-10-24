# CLI reference

```text
Usage: ferry [OPTIONS] <COMMAND>
```

| Command | Summary |
|---|---|
| [`init`](#ferry-init) | Write ferry.toml and report what this machine supports |
| [`base`](#ferry-base) | Build an immutable warm base that new agents fork from |
| [`spawn`](#ferry-spawn) | Create an agent: a git worktree plus a forked warm target dir |
| [`run`](#ferry-run) | Run a command in an agent's build context |
| [`shell`](#ferry-shell) | Start an interactive shell in an agent's build context |
| [`env`](#ferry-env) | Print an agent's paths and environment for wiring into agent tools |
| [`list`](#ferry-list) | List bases and agents with their status and disk usage |
| [`rm`](#ferry-rm) | Remove an agent's worktree and build state |
| [`gc`](#ferry-gc) | Delete bases that no agent uses and that are not current |
| [`doctor`](#ferry-doctor) | Check platform capabilities and print fixes for what is missing |
| [`shim`](#ferry-shim) | Print the directory holding Ferry's cargo shim |
| [`hook`](#ferry-hook) | Hook handlers for agent tools (reads the tool's JSON on stdin) |
| [`setup`](#ferry-setup-apparmor) | Change the system so Ferry's namespace sandbox works (asks first) |
| `help` | Print this message or the help of the given subcommand(s) |

## Global options

These work with every command, before or after it.

| Option | Meaning |
|---|---|
| `-C <DIR>` | Run as if ferry was started in DIR |
| `--json` | Print machine-readable JSON on stdout |
| `-h`, `--help` | Print help (`-h` prints a summary) |
| `-V`, `--version` | Print version |

Commands run against the git repository containing the current directory (or
the `-C` directory). Outside one, `doctor` still reports the machine-level
checks plus a failing `repository` check, and the `hook` handlers use the
directory named in their JSON input; every other command fails.

## Output, JSON and exit codes

Data goes to stdout; progress, warnings and errors go to stderr. Colour is
used only on a terminal. Errors look like this:

```text
error: no warm base exists for this repository
  hint: run `ferry base` to build one
```

with `caused by:` lines in between when there is an underlying error.

`--json` is accepted by every command. It switches `init`, `base`, `spawn`,
`env`, `list`, `rm`, `gc`, `doctor` (also with `--apparmor-profile`), `shim`
and `setup apparmor` to a single JSON document on stdout; the shapes are
listed under each command. Ferry is pre-1.0, so fields may still change between releases.
`--json` has no effect on `run` and `shell`, which replace themselves with
your command, or on the `hook` handlers, which speak the agent tool's
protocol.

Exit status is 0 on success and 1 on error. `doctor` exits 1 when a check
fails. `run` and `shell` exit with the status of the command they run.

## `ferry init`

```text
Usage: ferry init [OPTIONS]
```

Write ferry.toml and report what this machine supports.

| Option | Meaning |
|---|---|
| `--force` | Overwrite an existing ferry.toml |

Writes the default [configuration](configuration.md#the-default-file) to
`ferry.toml` in the root of the main checkout, then prints the same plan as
`ferry doctor`. Without `--force`, an existing file is left alone with a
warning.

JSON: `{ "config": <path>, "written": <bool>, "doctor": <doctor report> }`.

## `ferry base`

```text
Usage: ferry base [OPTIONS]
```

Build an immutable warm base that new agents fork from.

| Option | Meaning |
|---|---|
| `--rev <REV>` | Commit to build (any git revision) [default: HEAD] |
| `--cold` | Build from scratch instead of seeding from the newest compatible base |
| `--watch` | Keep running and build a base each time `--rev` moves to a new commit (not with `--cold`) |
| `--interval <SECS>` | With `--watch`: seconds between polls of `--rev` [default: 2] |
| `--gc` | With `--watch`: run `ferry gc` after each new base |

Checks out the commit into the store, runs the [warm commands](configuration.md#base-what-warms-a-base),
and makes the base current, so new agents fork from it. If a base with the
same commit and fingerprint already exists, it is made current without
building. Otherwise, unless `--cold` is given, the new base is seeded from the
newest ready base with the same fingerprint, so only what changed is rebuilt.
The seed must have the same manifests, lockfiles, Cargo configs, toolchain
files and Cargo flag variables, or its warm build must have been observed
(`[base] prune`, the default): the units the new build does not use are then
deleted, and the success line says how many (`pruned N units`). Build output
goes to stderr.

Warnings are printed when the namespace sandbox is unavailable (builds then
run without canonical paths), when a Cargo config above the repository (or an
untracked one in it) will not apply to Ferry's builds, when the warm
commands wrote files into the source tree, which agents will not have (build
scripts writing outside `OUT_DIR`), when clippy is not installed (the
generated clippy command is skipped), when the generated clippy command
failed (the base is kept), and when a refresh seeded across such changes was
rebuilt from scratch because Ferry could not tell which units the warm
commands used.

JSON: `{ "base": {...}, "reused": <bool>, "warnings": [...] }`, where `base`
has `id`, `commit`, `created_unix`, `build_secs`, `freshness`, `isolation`,
`toolchain`, `commands` (the ones that ran), `command_secs` (seconds per
command), `incomplete` (clippy commands that failed), `ferry_version`,
`dirtied`, `remap_paths`, `unit_inputs` (the hash seeding compares),
`seeded_from`, `observed` (Ferry saw every cargo call of the warm build) and
`pruned` (`{ "units", "bytes" }` deleted from the seed's units, or `null`).

### `ferry base --watch`

Runs in the foreground until interrupted (run it in a terminal tab, a
systemd or launchd user service, or a devcontainer's `postStartCommand`), so
the current base follows `--rev` (default `HEAD`; `--rev origin/main` follows
fetches; a revision that names no commit at startup is refused). It polls
the revision, and once a new commit has been seen on two polls in a row it
runs `ferry base --rev <commit>` in a child process. Polls
are skipped while a rebase, merge, cherry-pick, revert or bisect is in
progress. A build for a commit that fails keeps the previous base current and
is not retried until the revision moves again; a build is never cut short
when the revision moves, the newest commit is built next. Only one watcher
runs per repository; a second one is refused. With `--gc`, unused bases are
deleted after each new base, which matters on filesystems without reflinks,
where every base holds a full copy of the target dir.

JSON: one object per line, with `event` set to `building` (`commit`),
`built` (`commit`, `report`: what `ferry --json base` prints), `failed`
(`commit`, `error`), `collected` (`report`: what `ferry --json gc` prints)
or `gc-failed` (`error`).

## `ferry spawn`

```text
Usage: ferry spawn [OPTIONS] <NAME>
```

Create an agent: a git worktree plus a forked warm target dir.

| Argument or option | Meaning |
|---|---|
| `<NAME>` | Agent name (letters, digits, '-', '_', '.') |
| `--from <REF>` | Revision the agent's branch starts at [default: HEAD] |
| `--path <DIR>` | Where to create the worktree (default: the worktree directory, see `[agent] worktree-dir`) |
| `--branch <BRANCH>` | Branch name (default: the configured branch prefix plus NAME) |

Creates the branch and worktree, stamps the files that are identical to the
base, and forks the base's target dir. It never builds. The base is the
current one if it is at `--from`, else any ready base at that commit, else the
current base; files that differ from the chosen base are rebuilt on the first
build. Fails if the agent, the branch or a non-empty worktree directory
already exists, or if there is no base. A failure rolls back what the spawn
created: the worktree, the branch if it still points at `--from`, and the
fork.

Without `--path`, the worktree goes in the worktree parent directory (see
[Configuration](configuration.md#agent-worktrees-and-branches)), which is not
inside the store unless you configured it that way. A relative `--path` is
relative to the current directory. The directory must not exist or must be empty.

JSON: `{ "agent": {...}, "backend_reason": <string>, "fork": { "files", "bytes", "elapsed_secs" }, "elapsed_secs": <number>, "warnings": [...] }`,
where `agent` has `name`, `state`, `base`, `from`, `branch`, `worktree`,
`backend`, `isolation`, `freshness`, `created_unix`, `stamp` and `origin`.

## `ferry run`

```text
Usage: ferry run [OPTIONS] [NAME] -- <COMMAND>...
```

Run a command in an agent's build context.

Without NAME, uses the agent whose worktree contains the current directory.

| Argument | Meaning |
|---|---|
| `[NAME]` | The agent |
| `<COMMAND>...` | The command and its arguments, after `--` |

Enters the agent's sandbox and replaces itself with the command. The working
directory is the current directory if it is inside the agent's worktree, and
the worktree root otherwise; either way it is a host path. `PATH` starts with
the shim directory, and `FERRY_AGENT` and `FERRY_WORKTREE` are set. Every
`cargo` the command runs, directly or from scripts, goes through the shim and
builds in the agent's target dir.

```sh
ferry run a1 -- cargo test -p parser
ferry run a1 -- claude
ferry run -- cargo build      # inside a1's worktree: NAME comes from the directory
```

## `ferry shell`

```text
Usage: ferry shell [OPTIONS] [NAME]
```

Start an interactive shell in an agent's build context.

Like `ferry run [NAME] -- $SHELL` (or `/bin/sh` when `SHELL` is unset).
Without NAME, uses the agent whose worktree contains the current directory.

## `ferry env`

```text
Usage: ferry env [OPTIONS] [NAME]
```

Print an agent's paths and environment for wiring into agent tools.

Without NAME, uses the agent whose worktree contains the current directory.
Prints shell lines you can `eval`:

```sh
export FERRY_WORKTREE=/home/you/.local/share/ferry/worktrees/myproject-d5c4fd3a48754db4/a1
export PATH=/home/you/.cache/ferry/bin:"$PATH"
# cargo in /home/you/.local/share/ferry/worktrees/myproject-d5c4fd3a48754db4/a1 now builds in agent a1's sandbox; other commands: ferry run a1 -- <command>
```

It never prints `CARGO_TARGET_DIR`: agent tools pass their environment to
sub-agents working in other worktrees, and the shim picks the target dir per
call instead.

JSON: `{ "name", "worktree", "branch", "base", "backend", "isolation", "build_target_dir", "host_target_dir", "shim_dir", "run_prefix" }`.
`build_target_dir` is the target dir as builds see it (the canonical path in
the namespace sandbox). `host_target_dir` is the fork's host path for the
reflink and copy backends and `null` for overlay, whose target exists only
inside the sandbox. `run_prefix` is `["ferry", "run", <name>, "--"]`.

## `ferry list`

```text
Usage: ferry list [OPTIONS]
```

List bases and agents with their status and disk usage.

Bases: `*` marks the current base; `BUILT` is its age (or `incomplete`);
`DISK` its size; `USED BY` the number of agents forked from it. Agents:
`BASE` (with `(old)` when it is not the current base), `FORK` (backend),
`SANDBOX` (`namespace` or `none`), `DISK`, `STATUS` and `WORKTREE`.

`DISK` for an agent is the build state it holds beyond the base: the overlay
upper layer, or for reflink clones the files rewritten since the fork (shared
extents cannot be seen through `stat`), or the whole copy. `STATUS` is
`running` while any process started through `ferry run`, `ferry shell` or the
cargo shim for the agent is alive (JSON `busy`, with the pids Ferry can
attribute in `running`), `incomplete`
when spawning did not finish (remove it with `ferry rm`), `worktree missing`,
or `idle`.

JSON: `{ "bases": [...], "agents": [...] }`. A base has `id`, `ready`,
`current`, `meta`, `agents` and `disk_bytes`; an agent has the `spawn` agent
fields plus `base_is_current`, `worktree_exists`, `private_bytes` and
`busy` and `running` (PIDs).

## `ferry rm`

```text
Usage: ferry rm [OPTIONS] <NAME>
```

Remove an agent's worktree and build state.

Refuses if the worktree has uncommitted changes or processes are still running
in the agent's sandbox. The branch is kept if it has new commits.

| Option | Meaning |
|---|---|
| `--force` | Discard uncommitted changes and ignore running processes |

Untracked files count as uncommitted changes, except the `Cargo.lock` that
spawn copied from the base when the repository does not track one (as long as
it is unchanged). Processes started for the agent are detected on every
backend. The branch is deleted only if it still points at the
commit the agent started from; otherwise Ferry keeps it and says how many
commits it carries.

JSON: `{ "name", "worktree", "branch", "branch_deleted", "branch_commits", "rescued_ref" }`,
where `rescued_ref` names the ref that keeps commits a `--force` removal
found on no branch (otherwise `null`).

## `ferry gc`

```text
Usage: ferry gc [OPTIONS]
```

Delete bases that no agent uses and that are not current.

| Option | Meaning |
|---|---|
| `--dry-run` | Show what would be deleted |

First removes abandoned agents created by the Claude Code hook: clean
worktree, no new commits, nothing running, and not `git worktree lock`ed by
Claude Code. Agents created with `ferry spawn` are never removed. Then deletes
every base that is neither current nor used by an agent, including incomplete
ones left by interrupted builds, and skips bases that are being built.

The human-readable output lists reaped agents and removed and skipped bases.

JSON: `{ "removed": [...], "busy": [...], "kept": [...], "reaped_agents": [...], "warnings": [...], "freed_bytes": <number>, "dry_run": <bool> }`.

## `ferry doctor`

```text
Usage: ferry doctor [OPTIONS]
```

Check platform capabilities and print fixes for what is missing.

| Option | Meaning |
|---|---|
| `--apparmor-profile` | Print an `AppArmor` profile that allows this ferry binary to use user namespaces (Ubuntu 24.04+) |

Prints one line per check (`✓` ok, `·` information, `!` warning with a `fix:`
line, `✗` failure), then the plan: which fork backend agents will use and
whether their first edit can reuse incremental state. See
[Platform support](platforms.md#what-ferry-doctor-reports) for every check.

`--apparmor-profile` prints the profile for the binary's resolved path, and
warns on stderr if that path, or a directory above it, is writable by a
non-root user; it refuses paths with characters AppArmor reads as patterns.
With `--json` it prints `{ "binary", "profile", "warning" }`. See
[AppArmor on Ubuntu](troubleshooting.md#apparmor-on-ubuntu-2404-and-later).

JSON: `{ "checks": [{ "name", "status", "detail", "fix" }], "capabilities": {...}, "plan": {...} }`.
`capabilities` has `os`, `kernel`, `store_fs`, `reflink`, `userns`, `overlay`,
`sysctls` and `probe_label` (the namespace probe's AppArmor label, such as
`ferry (enforce)`, or `null`); `plan` has `backend`, `backend_reason`, `isolation`,
`freshness` and `incremental_reuse`.

## `ferry shim`

```text
Usage: ferry shim [OPTIONS]
```

Print the directory holding Ferry's cargo shim.

Put it first on PATH (`export PATH="$(ferry shim):$PATH"`): every cargo call
made inside an agent worktree then runs in that agent's sandbox with its own
target dir, and cargo elsewhere runs unchanged.

The directory is `<store parent>/bin`; `ferry shim` creates it and points its
`cargo` link at the running ferry binary. Run it inside a repository.

JSON: `{ "shim_dir": <path> }`.

## `ferry hook`

```text
Usage: ferry hook [OPTIONS] <COMMAND>
```

Hook handlers for agent tools (reads the tool's JSON on stdin).

| Command | Meaning |
|---|---|
| `claude-worktree-create` | Claude Code `WorktreeCreate`: spawn an agent, print its worktree path |
| `claude-worktree-remove` | Claude Code `WorktreeRemove`: remove the agent unless it has uncommitted work |
| `claude-session-start` | Claude Code `SessionStart`: put the cargo shim on PATH via `$CLAUDE_ENV_FILE` |

See [Claude Code](agents/claude-code.md#what-each-hook-does) for what each
handler reads and does.

## `ferry setup apparmor`

```text
Usage: ferry setup apparmor [OPTIONS]
```

Let ferry create user namespaces on Ubuntu 24.04+: install it to a root-owned
path and load its `AppArmor` profile, with one sudo call.

| Option | Meaning |
|---|---|
| `-y`, `--yes` | Run without asking; the plan is still printed |
| `--dry-run` | Print the plan and exit without changing anything (not with `--yes`) |
| `--uninstall` | Unload and remove Ferry's profile instead |
| `--bin-dir <DIR>` | Where to install ferry when its current path cannot be profiled (default `/usr/local/bin`) |

It first checks whether anything is needed. It does nothing, and exits 0, on
macOS, when the running ferry can already create user namespaces, when the
kernel has no `kernel.apparmor_restrict_unprivileged_userns` sysctl, and when
that sysctl is `0`. It refuses, and exits 1, when run as root, inside a Ferry
agent or a container, without the AppArmor 4.0 userspace, or when a file in
`/etc/apparmor.d` that Ferry did not write already defines a profile named
`ferry` or `unpriv_ferry`. It never reads the current repository or its
`ferry.toml`.

Otherwise it prints, on stderr: why the setup is needed (the sysctl, the
probe's failure and its AppArmor label), each step, what the profile allows,
the full profile, the root script and the exact `sudo` command. The steps,
each listed only when needed:

1. Install the running binary as `<bin-dir>/ferry`, owned by root with mode
   0755, unless it already sits at a root-owned path a profile can name or
   `<bin-dir>/ferry` is already a root-owned copy of it. A copy your user
   owns, or a symlink, is replaced. Root copies the binary from
   `/proc/<pid>/exe`, so nothing your user can write is copied.
2. Write `/etc/apparmor.d/<path with . for />` (for `/usr/local/bin/ferry`,
   `usr.local.bin.ferry`), check that it parses, and remove older Ferry
   profile files such as `/etc/apparmor.d/ferry`.
3. Load it with `apparmor_parser -r`; AppArmor loads it again at every boot.

Then, without sudo, it checks that the profiled binary can create a user
namespace, points the cargo shim at it, and warns if the first `ferry` on
`PATH` is another copy.

| Situation | What happens |
|---|---|
| stdin is a terminal, no `--yes` | asks `Run it? [y/N]`; only `y` or `yes` runs it |
| `--yes` | runs without asking; with stdin not a terminal, sudo gets `-n`, so a needed password fails at once |
| stdin is not a terminal, no `--yes` | refuses: `refusing to run sudo without confirmation: stdin is not a terminal` |
| `--json` without `--yes` | refuses, like stdin not being a terminal |
| `--dry-run` | prints the plan and exits 0 |

Running the upgraded copy is the upgrade path: after `cargo install`
upgrades `~/.cargo/bin/ferry`, run `~/.cargo/bin/ferry setup apparmor`. It
installs the new binary and leaves the profile alone. A bare `ferry setup
apparmor` with `/usr/local/bin` first on `PATH` runs the old installed copy.
That copy is set up already, so it reports `already-done` and says when
`~/.cargo/bin/ferry` is a different build. With
`--uninstall` it unloads and removes every Ferry profile file (one sudo call,
same confirmation rules); it leaves the installed binary and any
`/etc/apparmor.d/local/ferry` in place and says so.

JSON: `{ "action": "install" | "uninstall", "outcome", "reason", "binary",
"profile_path", "steps": [...], "command": [...], "verified": { "userns",
"overlay", "label" } | null, "warnings": [...] }`. `outcome` is one of
`not-applicable`, `not-needed`, `already-done`, `refused`, `planned` (a dry
run), `done` and `failed`; `command` is the full `sudo` argv. Exit status is 1
for `refused` and `failed`. See
[AppArmor on Ubuntu](troubleshooting.md#apparmor-on-ubuntu-2404-and-later).

## Ferry under other names

The ferry binary also runs as:

- **`cargo`**, through the shim link in `<store parent>/bin`. It resolves the
  agent from the working directory and runs the real cargo (the next `cargo`
  on `PATH` that is not Ferry) in that agent's sandbox, or unchanged outside
  agent worktrees. It exits with status 127 if no real cargo is found.
- **`ferry-rustc`**, a `RUSTC_WORKSPACE_WRAPPER` used only by the
  experimental [remap-paths mode](remap-paths.md).
