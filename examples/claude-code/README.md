# Ferry with Claude Code

Two ways to give Claude Code sessions their own warm, isolated Cargo build.
Both need a base first: run `ferry base` in the repository (and again when
your main branch moves on).

Full guide: <https://patwrall.github.io/ferry/agents/claude-code.html>

## R1: run the session in an agent's sandbox

```sh
ferry spawn a1
ferry run a1 -- claude
```

Claude starts in `a1`'s worktree, and every `cargo` it runs builds in `a1`'s
own target dir. Nothing to configure.

## R2: hooks, so every Claude worktree is a Ferry agent

[`settings.json`](settings.json) holds three hooks. Merge its `hooks` object
into `~/.claude/settings.json`:

| Hook | Command | What it does |
|---|---|---|
| `WorktreeCreate` | `ferry hook claude-worktree-create` | spawns a Ferry agent named after Claude's worktree and prints its path |
| `WorktreeRemove` | `ferry hook claude-worktree-remove` | removes the agent, unless it has uncommitted changes or running processes; keeps a branch with new commits |
| `SessionStart` | `ferry hook claude-session-start` | puts Ferry's cargo shim first on `PATH` through `$CLAUDE_ENV_FILE` (only `PATH`, never `CARGO_TARGET_DIR`) |

These fire for `claude --worktree <name>`, `EnterWorktree`, background
sessions and subagents with `isolation: worktree`:

```sh
claude --worktree feature-x
```

A project's `.claude/settings.json` also works, but only once it is committed:
Claude Code reads a hook-created worktree's project settings from that
worktree's checkout.

Verified with Claude Code 2.1.280: the hook created the Ferry agent, the
session ran in its worktree, the shim was on `PATH`, and `cargo build` in the
session was warm (0.04 s).

## Subagents

[`agents/rust-implementer.md`](agents/rust-implementer.md) is a subagent with
`isolation: worktree` in its frontmatter. Copy it to `.claude/agents/` (or
`~/.claude/agents/`). With the R2 hooks installed, each run gets a Ferry agent
named `agent-<hex>`.

Claude Code does not run `WorktreeRemove` for subagent worktrees or for
`claude -p` sessions. `ferry gc` removes hook-created agents that are clean,
have no new commits and are idle; `ferry gc --dry-run` shows which.

## Claude Code's sandbox

If you enable Claude Code's sandbox, allow writes to Ferry's store (the parent
of the directory `ferry shim` prints, by default `~/.cache/ferry` on Linux).
Claude Code 2.1.305 and later also block the user namespaces the shim needs;
use R1, `sandbox.allowNestedUserNamespaces`, or `sandbox.excludedCommands`.
See the guide for the exact settings and their trade-offs.

## Do not

- set `worktree.symlinkDirectories: ["target"]`;
- put `CARGO_TARGET_DIR` in the settings `env` block or in `$CLAUDE_ENV_FILE`.

Either makes agents share one target dir, which gives one agent another's
code with nothing rebuilt.
