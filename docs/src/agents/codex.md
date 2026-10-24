# Codex

Codex CLI has no hook that lets another tool create its worktrees, so Ferry
creates the worktree and Codex is started in it. Do not use
`codex --worktree`: it creates its own worktree under `$CODEX_HOME/worktrees`,
which has no Ferry fork.

Example files are in
[`examples/codex/`](https://github.com/patwrall/ferry/tree/main/examples/codex).

## Start Codex in a Ferry agent

```sh
ferry spawn a1
ferry run a1 -- codex --cd ~/.local/share/ferry/worktrees/myproject-d5c4fd3a48754db4/a1
```

`ferry run` puts the cargo shim first on `PATH` for Codex and everything it
starts, so every `cargo` Codex runs builds in `a1`'s target dir. Codex's own
sandbox works inside Ferry's namespace.

The launcher script
[`examples/codex/ferry-codex.sh`](https://github.com/patwrall/ferry/blob/main/examples/codex/ferry-codex.sh)
does this in one step, creating the agent when it does not exist:

```sh
ferry-codex.sh a1                               # interactive
ferry-codex.sh a1 exec "make the parser tests pass"
```

Run it inside the repository, after `ferry base`. It reads the worktree path
from `ferry env` and ends with
`exec ferry run "$name" -- codex --cd "$worktree" "$@"`.

If you prefer to start Codex yourself, export the shim before starting it:

```sh
eval "$(ferry env a1)"         # exports FERRY_WORKTREE, puts the shim first on PATH
codex --cd "$FERRY_WORKTREE"
```

Codex has no equivalent of Claude Code's environment file, and
`shell_environment_policy.set` only takes literal values, so set `PATH`
before Codex starts. Never set `CARGO_TARGET_DIR` there.

## Sandbox settings

In `workspace-write` mode, Codex lets commands write to the working directory
and temp dirs. cargo running through Ferry also writes to the agent's fork and
to Ferry's canonical mount points. Both live under Ferry's store, outside the
worktree, so without more configuration builds fail with:

```text
error: failed to open: /home/you/.cache/ferry/myproject-d5c4fd3a48754db4/mnt/target/debug/.cargo-build-lock

Caused by:
  Read-only file system (os error 30)
```

Make the store's parent directory a writable root in `~/.codex/config.toml`
([`examples/codex/config.toml`](https://github.com/patwrall/ferry/blob/main/examples/codex/config.toml)):

```toml
sandbox_mode = "workspace-write"

[sandbox_workspace_write]
writable_roots = ["/home/you/.cache/ferry"]
```

or pass it per session with `codex --add-dir /home/you/.cache/ferry`.

To find the path, run `dirname "$(ferry shim)"` inside the repository. It is
`$XDG_CACHE_HOME/ferry` (usually `~/.cache/ferry`) on Linux and
`~/Library/Caches/ferry` on macOS, unless you set `FERRY_HOME` or `[store] dir`.
`ferry --json env a1` shows the related paths: `shim_dir` is
`<store parent>/bin`, and `build_target_dir` is under the store.

Status: the read-only failure above was reproduced with `codex sandbox`
(Codex CLI 0.162.0) inside `ferry run`. The `writable_roots` and `--add-dir`
settings come from Codex's configuration reference and were not exercised in a
live Codex session.

Codex also keeps `.git` inside writable roots read-only, and that includes the
git directory a worktree points to, so `git commit` from inside Codex's
sandbox needs approval to run outside it.

## Several Codex sessions in parallel

Give each session its own agent:

```sh
ferry-codex.sh parser    # terminal 1
ferry-codex.sh cli       # terminal 2
```

Each one builds warm, in its own target dir, and neither can change the
other's build. Remove an agent with `ferry rm <name>` when you have merged or
dropped its branch.
