# Ferry for AI agents

Ferry gives every coding agent working on a Rust repository its own git
worktree with an already-warm, forked Cargo target dir. If your user asked you
to install or set up Ferry, follow the first part. If you are changing Ferry's
own code, follow the second.

Every command takes `--json` (before the subcommand: `ferry --json doctor`)
for output you can parse.

## Install and set up Ferry for your user

### 1. Install

Linux (x86-64, ARM64) or macOS. Pick one:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/patwrall/ferry/releases/latest/download/ferry-rs-installer.sh | sh
brew install patwrall/tap/ferry
cargo install ferry-rs --locked
```

The installer puts `ferry` in `$CARGO_HOME/bin` (`~/.cargo/bin`). Check with
`ferry --version`.

### 2. Check the machine

Run `ferry doctor` and read what it reports. It says which fork backend and
sandbox Ferry will use, and how to fix anything missing.

- On Ubuntu 24.04 and later, user namespaces are restricted until
  `ferry setup apparmor` installs Ferry's AppArmor profile. It runs one
  `sudo` command. **Ask your user before running it**, and show them the
  command it prints.
- Never run other `sudo` commands that `ferry doctor` suggests without your
  user's approval.

### 3. Build a base

In the user's repository (the main checkout):

```sh
ferry base
```

This builds the current commit once, as long as a cold `cargo build` takes,
and warms `build`, `test`, `check` and `clippy`. Run it again after the main
branch moves, or run `ferry base --watch` in the background to follow `HEAD`.

### 4. Give each agent its own worktree

```sh
ferry spawn a1                      # worktree, branch ferry/a1 and a forked warm target
ferry run a1 -- cargo test          # any command, inside a1's build context
eval "$(ferry env a1)" && cd "$FERRY_WORKTREE"   # or: plain cargo, routed by the shim
```

`ferry list` shows bases and agents, `ferry rm a1` removes one (it refuses
when the worktree has uncommitted changes), and `ferry gc` deletes unused
bases.

### 5. Connect the user's agent tool

- **Claude Code:** add the `WorktreeCreate`, `WorktreeRemove` and
  `SessionStart` hooks from
  [`examples/claude-code/settings.json`](examples/claude-code/settings.json)
  to `~/.claude/settings.json`. Merge them with any hooks already there; do
  not replace the file. Ask your user before editing it. Then
  `claude --worktree`, background sessions and subagents with
  `isolation: worktree` each get a Ferry agent. Claude Code's own sandbox can
  block the user namespace Ferry needs; see
  [Claude Code](docs/src/agents/claude-code.md).
- **Codex:** see [`examples/codex/ferry-codex.sh`](examples/codex/ferry-codex.sh).
- **Anything else:** start the agent with `ferry run a1 -- <agent>`.

### Rules

- **Never export `CARGO_TARGET_DIR` or `CARGO_BUILD_BUILD_DIR`** into an
  agent session or a settings file. Agent tools copy their environment into
  sub-agents in other worktrees, which then share one target dir and can run
  each other's code. Ferry's shim picks the right target from the working
  directory instead.
- **Ferry is not a security sandbox.** It isolates build state only.
- When a build in an agent seems wrong, compare it with a cold build and
  report it at <https://github.com/patwrall/ferry/issues>.

Full documentation: <https://patwrall.github.io/ferry>
([agent integrations](https://patwrall.github.io/ferry/agents.html),
[troubleshooting](https://patwrall.github.io/ferry/troubleshooting.html)).

## Working on Ferry itself

Read [`docs/src/architecture.md`](docs/src/architecture.md) before changing
code: it lists the invariants every change must keep. Then follow
[`CONTRIBUTING.md`](CONTRIBUTING.md). The essentials:

- Run before pushing: `cargo fmt --all --check`,
  `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked` and `typos`.
- Unset `CARGO_INCREMENTAL` and `CI` before running the tests; the tests
  exercise incremental reuse. Tests that need a missing capability (user
  namespaces, overlayfs, reflinks) skip themselves; set
  `FERRY_TEST_REQUIRE=userns,overlay` to make them fail instead.
- Commits and PR titles follow Conventional Commits. Do not edit
  `CHANGELOG.md`; release-plz generates it. Changes reach `main` through pull
  requests, and every user-visible change updates `docs/`.
