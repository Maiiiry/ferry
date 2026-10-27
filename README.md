<h1 align="center">Ferry</h1>

<h3 align="center">Blazingly fast Rust worktrees for AI agents.</h3>

<p align="center">Every agent starts from one warm Cargo build.<br>No cold compiles, no crossed builds.</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#try-it">Try it</a> ·
  <a href="#how-fast">How fast</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="https://patwrall.github.io/ferry">Docs</a>
</p>

<p align="center">
  <a href="https://github.com/patwrall/ferry/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/patwrall/ferry/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://crates.io/crates/ferry-rs"><img alt="crates.io" src="https://img.shields.io/crates/v/ferry-rs.svg"></a>
  <a href="#license"><img alt="license" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue"></a>
</p>

<p align="center">
  <img src="docs/src/demo.gif" alt="A new git worktree builds ripgrep cold in about 8 s; a Ferry agent spawns in under a second, builds in 0.1 s and rebuilds an edit in about 1.3 s" width="760">
</p>

> **Are you an AI agent?** Read [AGENTS.md](AGENTS.md): how to install Ferry
> and set it up for your user.

## Why

Running several coding agents on one Rust repository means one git worktree
per agent, and every new worktree's first `cargo build` starts cold. Pointing
them all at one `CARGO_TARGET_DIR` instead makes builds queue on Cargo's lock,
and it is **silently wrong**: an agent can build, rebuild 0 crates, and run
another agent's code. Ferry's tests reproduce that with plain cargo in 60 of
60 runs ([`tests/contamination.rs`](tests/contamination.rs)).

Ferry builds each commit once, then gives every agent its own fork of that
warm build.

## Install

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/patwrall/ferry/releases/latest/download/ferry-rs-installer.sh | sh
```

A prebuilt binary of about 1 MB for Linux (x86-64, ARM64) and macOS. Or:

```sh
brew install patwrall/tap/ferry
cargo install ferry-rs --locked
```

On Ubuntu 24.04 and later, run `ferry setup apparmor` once (it shows each step
and asks before its one `sudo` call).

## Try it

In any Rust repository:

```sh
ferry base                    # build one warm base for HEAD, once per commit
ferry spawn a1                # worktree + branch ferry/a1 + forked warm target, in under a second
ferry run a1 -- cargo test    # warm from the first build
```

Spawn as many agents as you like; each builds in its own fork. Run your agent
inside one with `ferry run a1 -- claude`, or let Claude Code create Ferry
agents for `--worktree` sessions and subagents through its hooks
([example](examples/claude-code/settings.json)). `ferry doctor` says what your
machine supports.

## How fast

ripgrep, a new agent's first edit and build:

<!-- bench:start -->
| New agent, then one edit | Time | |
|---|---:|---:|
| New git worktree, cold | 7.06 s | |
| Copied warm target dir | 3.67 s | |
| kache, warm cache | 2.86 s | |
| **Ferry** | **1.13 s** | **6.2× faster than cold** |
<!-- bench:end -->

- The agent's first build without an edit: **0.06 s**, 0 crates compiled.
- Three agents with different edits building at once: **1.41 s**, against
  6.66 s for three worktrees sharing one target dir.

Measured on a 6-core Ryzen 3600X, btrfs, rustc 1.99. Every scenario, step and
machine detail: [benchmarks](https://patwrall.github.io/ferry/benchmarks.html).

## How it works

`ferry base` builds one commit's workspace once and never touches it again.
`ferry spawn` gives each agent a copy-on-write clone of that target dir
(btrfs, XFS, APFS), an overlayfs layer, or a copy, and stamps the files that
are provably identical to the base so Cargo sees them as fresh. On Linux every
build runs in a private mount namespace where all agents' worktrees and target
dirs appear at the same two paths, so rustc's incremental state stays valid
and an agent's first edit compiles incrementally.
[More](https://patwrall.github.io/ferry/how-it-works.html).

## Good to know

- **Linux gets the full speed-up.** It needs unprivileged user namespaces,
  which most distributions allow. On macOS agents reuse every unchanged crate,
  but an edited crate recompiles from scratch.
- **It is not a security sandbox.** It isolates build state; agents still run
  as you, with your files and network.
- **Local only.** Ferry shares builds between agents on one machine, not
  across machines.
- **It's young.** Tell us when an agent's build differs from a cold one:
  [issues](https://github.com/patwrall/ferry/issues).

All the details: [platforms](https://patwrall.github.io/ferry/platforms.html)
and [limitations](https://patwrall.github.io/ferry/faq.html#what-are-ferrys-limitations).

## Learn more

- [Getting started](https://patwrall.github.io/ferry/getting-started.html): a full walkthrough
- [Agent integrations](https://patwrall.github.io/ferry/agents.html): Claude Code, Codex, anything else
- [Configuration](https://patwrall.github.io/ferry/configuration.html) and [CLI reference](https://patwrall.github.io/ferry/cli.html)
- [FAQ](https://patwrall.github.io/ferry/faq.html): how it compares with sccache, kache and plain worktrees
- [Troubleshooting](https://patwrall.github.io/ferry/troubleshooting.html)
- [Architecture](https://patwrall.github.io/ferry/architecture.html): the invariants and how the code keeps them

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Security issues: [`SECURITY.md`](SECURITY.md).

## License

MIT or Apache-2.0, at your option ([`LICENSE-MIT`](LICENSE-MIT),
[`LICENSE-APACHE`](LICENSE-APACHE)). Unless you explicitly state otherwise,
any contribution you submit for inclusion is dual licensed as above, without
any additional terms or conditions.
