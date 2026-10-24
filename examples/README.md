# Examples

Working examples for using Ferry with agent tools and configuring it. Each
one was run against a throwaway repository; the files say what was and was not
verified.

| Path | What |
|---|---|
| [`claude-code/`](claude-code/) | `~/.claude/settings.json` hooks (`WorktreeCreate`, `WorktreeRemove`, `SessionStart`) and a subagent with `isolation: worktree` |
| [`codex/`](codex/) | a launcher that starts Codex in a Ferry agent, and the `config.toml` sandbox settings |
| [`generic/parallel-agents.sh`](generic/parallel-agents.sh) | spawn N agents and run one command in each concurrently |
| [`config/`](config/) | annotated `ferry.toml` variants: CI-style checks, release profile, custom commands, a workspace in a subdirectory, platform overrides |

To try a `ferry.toml` variant without copying it, point `FERRY_CONFIG` at it
from inside your repository:

```sh
FERRY_CONFIG=/path/to/ferry/examples/config/ci-checks.toml ferry doctor
```

Documentation: <https://patwrall.github.io/ferry>
