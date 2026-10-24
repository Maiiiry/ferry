# Agent integrations

Ferry works with any agent that can be started in a directory. This page has
the rules every integration follows and a generic recipe. The next pages cover
[Claude Code](agents/claude-code.md) and [Codex](agents/codex.md) in detail.
Working example files are in
[`examples/`](https://github.com/patwrall/ferry/tree/main/examples).

## The rules

1. **One Ferry agent per coding agent.** Each gets its own worktree, branch and
   fork. Create it with `ferry spawn <name>` or, for Claude Code, let its
   `WorktreeCreate` hook do it.
2. **Route cargo through Ferry.** Either run the whole agent under
   `ferry run <name> -- <agent>`, or put the shim first on `PATH`
   (`export PATH="$(ferry shim):$PATH"`). Both make every `cargo` the agent
   runs build in its own sandbox and target dir.
3. **Never export `CARGO_TARGET_DIR` into an agent session.** Agent tools copy
   their environment into sub-agents, including sub-agents that work in other
   worktrees. An exported target dir sends all of them to one shared target:
   the contamination bug Ferry exists to prevent. The same goes for
   `CARGO_BUILD_BUILD_DIR`, a `target-dir` in a settings `env` block, and
   symlinking `target/` between worktrees. The shim exists so that nothing
   needs to be exported: it works out the agent from the directory cargo runs
   in, on every call.
4. **Start the agent at the host worktree path**, not at Ferry's canonical
   mount path. Agent tools key transcripts and memory on the working
   directory; the canonical path is the same for every agent, so they would
   all share one history. `ferry run` keeps host paths and only moves cargo to
   the canonical path.

## Generic recipe

For any agent that takes a working directory and inherits the environment:

```sh
ferry spawn a1
ferry run a1 -- my-agent           # the whole agent in a1's build context
```

`ferry run` starts the command in the agent's worktree when your current
directory is outside it. Inside the session, `PATH` has the shim first, and
`FERRY_AGENT` and `FERRY_WORKTREE` name the agent and its worktree for your
scripts.

If the agent cannot be started through `ferry run`, start it however you
normally would in the worktree, with the shim first on `PATH`.
`ferry env a1` prints the two lines that set this up:

```sh
eval "$(ferry env a1)"             # exports FERRY_WORKTREE, puts the shim first on PATH
cd "$FERRY_WORKTREE"
my-agent
```

`ferry env` deliberately does not print `CARGO_TARGET_DIR`. For scripts,
`ferry --json env a1` prints the worktree, branch, shim directory and target
paths as JSON.

[`examples/generic/parallel-agents.sh`](https://github.com/patwrall/ferry/blob/main/examples/generic/parallel-agents.sh)
spawns N agents, runs a command in each concurrently and reports the results.

## Agents with their own sandbox

Agent tools often run commands in a sandbox that limits writes to the working
directory. Ferry's build state lives elsewhere, in the store, so the sandbox
must also allow writes to the store's parent directory (by default
`~/.cache/ferry` on Linux). It is the parent of the directory `ferry shim`
prints. All of Ferry's writable state, including overlay upper layers and the
canonical mount points, is under it.

On Linux, the shim also has to create a user namespace. Some agent sandboxes
block that; then either run the agent under `ferry run`, which creates the
namespace before the agent's sandbox starts, or let cargo bypass the agent's
sandbox. The Claude Code and Codex pages give the exact settings.

## Other agents

Cursor, Gemini CLI, OpenCode, Aider and Amp all start in a working directory
and inherit the environment, so the generic recipe applies to them. These
combinations have not been tested with Ferry. Do not use an agent's own
worktree feature to create the worktree unless it lets Ferry create it (as
Claude Code's `WorktreeCreate` hook does): a worktree Ferry did not create has
no fork, and the shim runs plain cargo there.
