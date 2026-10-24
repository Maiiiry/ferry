# Ferry with Codex

Codex CLI cannot be told to use a worktree created by another tool, so Ferry
creates the worktree and Codex starts in it, inside the agent's build context.
Do not use `codex --worktree`: its worktrees have no Ferry fork.

Full guide: <https://patwrall.github.io/ferry/agents/codex.html>

## The launcher

[`ferry-codex.sh`](ferry-codex.sh) creates the agent if it does not exist,
then runs `exec ferry run "$name" -- codex --cd "$worktree" "$@"`. Run it
inside the repository, after `ferry base`:

```sh
./ferry-codex.sh a1                                  # interactive session
./ferry-codex.sh a1 exec "make the parser tests pass"
./ferry-codex.sh a2                                  # a second, parallel session
```

Inside the session, Ferry's cargo shim is first on `PATH`, so every `cargo`
Codex runs builds in that agent's own warm target dir. The script never sets
`CARGO_TARGET_DIR`.

## Sandbox settings

With `sandbox_mode = "workspace-write"`, Codex lets commands write only to the
working directory and temp dirs. cargo through Ferry also writes to Ferry's
store, so add the store's parent directory as a writable root. Merge
[`config.toml`](config.toml) into `~/.codex/config.toml`, with your real path:

```toml
sandbox_mode = "workspace-write"

[sandbox_workspace_write]
writable_roots = ["/home/you/.cache/ferry"]
```

Find the path with `dirname "$(ferry shim)"` inside the repository, or look at
`ferry --json env a1`: `shim_dir` is `<store parent>/bin`. The default is
`$XDG_CACHE_HOME/ferry` (`~/.cache/ferry`) on Linux and `~/Library/Caches/ferry`
on macOS, or `$FERRY_HOME` when set. `codex --add-dir <path>` does the same for
one session.

Without it, cargo fails with
`failed to open: .../mnt/target/debug/.cargo-build-lock ... Read-only file system`.
That failure was reproduced with `codex sandbox` (Codex CLI 0.162.0) inside
`ferry run`; the `writable_roots` and `--add-dir` settings follow Codex's
configuration reference and were not tested in a live Codex session.
