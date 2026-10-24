# Agent integrations: worktrees, hooks, sandboxes, env

Research brief for Ferry, 2026-10-23. Goal: concrete ways to plug `ferry spawn/run/env/rm`
into Claude Code, Codex and other agents without inventing APIs.

**Versions.** Claude Code docs as of today (changelog head is **2.1.310**). Local binary is
**2.1.280**, and every "measured" Claude result below comes from 2.1.280. Codex CLI is
**0.162.0** locally, with docs at learn.chatgpt.com (developers.openai.com/codex/llms.txt
now points there). `@anthropic-ai/sandbox-runtime` is **0.0.81** (anthropics/sandbox-runtime
@ c66fb77). Kernel is 6.18 and bubblewrap is 0.12.0.

**Method.** Claude probes ran as `claude -p --model haiku` in a throwaway repo under the
scratchpad. They used `--setting-sources project|local` so the user's own hooks stayed out.
Codex probes used `codex exec` and `codex sandbox`. Scripts and logs live in
`$SCRATCH/research-agents/`.

---

## TL;DR

1. **Claude Code has the exact hook Ferry needs.** `WorktreeCreate` *replaces* git worktree
   creation (for `--worktree`, `EnterWorktree`, `isolation: worktree` subagents and more) and
   uses the path printed on the last stdout line (§1c, measured).
2. **Never export `CARGO_TARGET_DIR` via `CLAUDE_ENV_FILE`.** The main session's env file
   also reaches worktree-isolated subagents (measured), recreating the shared-target bug.
   Resolve agent and target **from `$PWD`**.
3. **Wrap cargo, not the shell.** `CLAUDE_CODE_SHELL_PREFIX` would break Claude's
   `pwd -P` cwd tracking under remapped paths (§1d). A `cargo` PATH shim is the safe
   interception point; `PreToolUse` rewrites work but miss cargo called from scripts.
4. **Nested userns inside agent sandboxes:** Codex allows it (measured, §2). Claude Code
   ≥2.1.305 / srt 0.0.81 blocks it by default unless `sandbox.allowNestedUserNamespaces: true`
   (measured with srt, §1e). Local Claude 2.1.280 still allowed it (measured).
5. **The reverse always works.** Both Codex's sandbox and srt run fine *inside* a Ferry
   user+mount namespace (measured), so `ferry run a1 -- claude|codex` composes with the agent
   sandbox left on.
6. **Map own uid, not root.** Claude refuses `--dangerously-skip-permissions` as root. A Rust
   `unshare(NEWUSER|NEWNS)` + `uid_map "1000 1000 1"` + direct `mount(2)` works (measured);
   util-linux `mount` refuses in that mode.

---

## 1. Claude Code

### 1a. Built-in worktrees: VERIFIED
- `claude --worktree <name>` (or `-w`) creates `<repo>/.claude/worktrees/<name>/` on branch
  `worktree-<name>`. With no name it generates one such as `bright-running-fox`. `#1234` or a
  PR/MR URL branches from that PR. `--tmux` requires `--worktree`.
  [cli-reference](https://code.claude.com/docs/en/cli-reference),
  [worktrees](https://code.claude.com/docs/en/worktrees#start-claude-in-a-worktree)
- The base comes from `worktree.baseRef`: `"fresh"` (default, `origin/<default>`) or
  `"head"`. A branch name is not accepted. Other keys: `worktree.symlinkDirectories`,
  `worktree.sparsePaths` and `worktree.bgIsolation`. `worktree.location` is desktop/SSH only.
  [settings-reference#worktree](https://code.claude.com/docs/en/settings-reference#worktree)
- `.worktreeinclude` (gitignore syntax) copies ignored files into git-created worktrees. It is
  **not processed when a `WorktreeCreate` hook is configured.**
  [worktrees#copy-gitignored-files](https://code.claude.com/docs/en/worktrees#copy-gitignored-files-into-worktrees)
- Cleanup: on interactive exit a clean, unnamed session's worktree and branch are removed;
  otherwise Claude prompts. `-p` runs never clean up. A periodic sweep removes subagent and
  background worktrees older than `cleanupPeriodDays`, but only those with Claude's git
  marker, so hook-created ones are never swept. Claude holds `git worktree lock` while an
  agent runs.
  [worktrees#clean-up-worktrees](https://code.claude.com/docs/en/worktrees#clean-up-worktrees)
- Isolation enforcement: Claude blocks Edit/Write into the main checkout, Bash whose cwd
  resolves to the main checkout, and git redirects (`git -C`, `GIT_DIR`, ...). The check reads
  the command text only, so it does not see what a shim does internally.
  [worktrees#how-claude-code-enforces-isolation](https://code.claude.com/docs/en/worktrees#how-claude-code-enforces-isolation)
- Workspace trust is keyed on the main checkout's root even for worktrees.
  [permissions](https://code.claude.com/docs/en/permissions)
- **Hazard.** `worktree.symlinkDirectories: ["target"]` would make every worktree share one
  target dir: the shared-target bug.

### 1b. Subagents with `isolation: worktree`: VERIFIED
- Frontmatter field `isolation`, value `worktree`. It is also accepted in `--agents` JSON;
  measured, with `tools` given as an array. A worktree with no changes is removed when the
  subagent finishes.
  [sub-agents#supported-frontmatter-fields](https://code.claude.com/docs/en/sub-agents#supported-frontmatter-fields)
- The base defaults to the default branch, not the parent's HEAD, unless
  `worktree.baseRef: "head"` is set.
  [worktrees#isolate-subagents](https://code.claude.com/docs/en/worktrees#isolate-subagents-with-worktrees)
- The subagent's Bash runs inside its worktree, and `cd` doesn't persist between subagent
  calls. [sub-agents](https://code.claude.com/docs/en/sub-agents)
- **Measured:** with a WorktreeCreate hook, a subagent spawn fires the hook with
  `"name": "agent-<17 hex>"` (e.g. `agent-abd7add2abe2a458a`). `agent_id`/`agent_type` are
  absent from that input. The hook-created worktree is still there after the subagent ends,
  matching the doc: "When a subagent or a workflow agent finishes, Claude Code keeps the
  worktree your WorktreeCreate hook made for it and doesn't run your WorktreeRemove hook."
  [hooks#worktreeremove](https://code.claude.com/docs/en/hooks#worktreeremove)

### 1c. Hooks: VERIFIED ([hooks reference](https://code.claude.com/docs/en/hooks))

**Events:** `SessionStart`, `Setup`, `UserPromptSubmit`, `UserPromptExpansion`, `PreToolUse`,
`PermissionRequest`, `PermissionDenied`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`,
`Notification`, `MessageDisplay`, `SubagentStart`, `SubagentStop`, `TaskCreated`,
`TaskCompleted`, `Stop`, `StopFailure`, `TeammateIdle`, `InstructionsLoaded`, `ConfigChange`,
`CwdChanged`, `DirectoryAdded`, `FileChanged`, `WorktreeCreate`, `WorktreeRemove`,
`PreCompact`, `PostCompact`, `PreModelSwitch`, `PostModelSwitch`, `Elicitation`,
`ElicitationResult`, `SessionEnd`.

**Locations** (entries merge across levels): `~/.claude/settings.json`,
`.claude/settings.json`, `.claude/settings.local.json`, managed policy, plugin
`hooks/hooks.json`, and skill or subagent frontmatter.

**Matchers:** `"*"`, `""` or omitted matches everything. A value of only letters, digits,
`_`, `-`, spaces, `,` and `|` is an exact string or list; anything else is an unanchored JS
regex. `WorktreeCreate`, `WorktreeRemove` and `CwdChanged` have **no matcher support**.

**Handler fields:** `type` (`command`, `http`, `mcp_tool`, `prompt`, `agent`); `if` (a
permission-rule filter, tool events only); `timeout` in seconds (default **600** for
command, http and mcp_tool); `command` (plus `args` for exec form); `async`, `asyncRewake`,
`shell`; `onFailure` (`continue` or `block`, v2.1.295+).

**WorktreeCreate**
- Input = common fields + `name`. Measured stdin:
  `{"session_id","transcript_path","cwd":<main checkout>,"hook_event_name":"WorktreeCreate","name":"t1"}`.
- Output: a command hook prints the path as the **last non-empty stdout line**; ANSI is
  stripped, and everything else goes to stderr. An HTTP hook returns
  `{"hookSpecificOutput":{"hookEventName":"WorktreeCreate","worktreePath":"/abs"}}`.
  Any non-zero exit fails creation.
- Claude refuses absolute paths containing `.`/`..` and paths that pass through a symlink
  below the repo root.
- A directory *without* git metadata must sit outside any repository, or Claude refuses it.
  **Measured:** proper `git worktree add` checkouts are accepted both outside the repo and
  inside it (`<repo>/.ferry/agents/t3/src`). The session's cwd became the returned path, on
  the hook's branch (`ferry/t1`).
- The hook replaces git entirely, which is why `.worktreeinclude` is skipped.
  [hooks#worktreecreate](https://code.claude.com/docs/en/hooks#worktreecreate)

**WorktreeRemove**
- Input adds `worktree_path`. Exit 0 counts as removed; non-zero means the removal failed
  *if the directory still exists*. JSON output is discarded. It fires on interactive
  exit/remove, `ExitWorktree` (only with `discard_changes: true` for hook worktrees),
  background-session delete, and `remote-control` cleanup.
- It does **not** fire for subagent or workflow worktrees, and nothing cleans up in `-p`
  mode. Claude never deletes the hook's branch and checks dirtiness only via git, so the
  hook must delete the branch itself and refuse to delete unsaved work.
  [hooks#worktreeremove](https://code.claude.com/docs/en/hooks#worktreeremove)

**Ordering (measured).** `WorktreeCreate` runs first, from the *launch* checkout's
settings. `SessionStart` then runs with `cwd` = the worktree and loads project settings
**from the worktree's checkout**: an *uncommitted* `.claude/settings.json` in the main repo
did **not** apply in the hook-created worktree, and once committed it did. Ferry's hooks
therefore belong in `~/.claude/settings.json` or a committed `.claude/settings.json`.

**PreToolUse (Bash)**
- Input `tool_input` has `command`, plus optional `description`, `timeout` and
  `run_in_background`. Measured keys: `cwd, hook_event_name, permission_mode, prompt_id,
  session_id, tool_input, tool_name, tool_use_id, transcript_path`.
- Output:
  `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow|deny|ask|defer","permissionDecisionReason":"...","updatedInput":{...},"additionalContext":"..."}}`.
- `updatedInput` **replaces the whole input object**, and permission rules are evaluated
  against the rewritten input. If several hooks rewrite, the last to finish wins
  (non-deterministic). [hooks-guide](https://code.claude.com/docs/en/hooks-guide)
- **Measured:** rewriting `cargo test -p foo` to `echo VIA-FERRY-…` executed the rewritten
  command. This worked with `"permissionDecision":"allow"` and also with no decision; in the
  second case the allow rule `Bash(echo *)` matched the *rewritten* command.

**SessionStart, CLAUDE_ENV_FILE and the `env` key**
- `CLAUDE_ENV_FILE` is available to `SessionStart`, `Setup`, `CwdChanged` and `FileChanged`.
  Append `export` lines to it, and its contents run before **each Bash command** (so
  `export PATH="$X:$PATH"` works). `CwdChanged` clears what that hook wrote on the next
  CwdChanged.
  [hooks#persist-environment-variables](https://code.claude.com/docs/en/hooks#persist-environment-variables)
- **Measured:** the env file lived at `~/.claude/session-env/<session-id>/sessionstart-hook-0.sh`.
  Both a PATH-prepended `cargo` shim and `CARGO_TARGET_DIR` reached Bash. They reached the
  **isolated subagent's** Bash too, which is the hazard in TL;DR #2.
- The settings `"env"` key sets literal values for every session and subprocess, with no
  `$PATH` expansion documented. It overrides shell exports.
  [settings-reference#env](https://code.claude.com/docs/en/settings-reference#env)

### 1d. Bash tool: VERIFIED + measured
- Each command runs in a separate process. `cd` carries over only while it stays inside the
  project or added dirs; otherwise Claude resets it ("Shell cwd was reset to <dir>"). Env
  vars do not persist between commands. Shell snapshots of rc-file aliases and functions
  are applied.
  [tools-reference#bash-tool-behavior](https://code.claude.com/docs/en/tools-reference#bash-tool-behavior)
- `CLAUDE_CODE_SHELL` picks the shell (bash or zsh only).
  `CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR=1` disables cwd carry-over.
- `CLAUDE_CODE_SHELL_PREFIX` wraps Bash calls, hooks, the status line and stdio MCP startup.
  [env-vars](https://code.claude.com/docs/en/env-vars)
- **Measured `$1`:**
  `source ~/.claude/shell-snapshots/snapshot-zsh-….sh … && eval 'cd sub && pwd' < /dev/null && pwd -P >| /tmp/claude-378a-cwd`.
- Default timeout is 2 min, up to 10 min (`BASH_DEFAULT_TIMEOUT_MS`/`BASH_MAX_TIMEOUT_MS`).
  On timeout a foreground command moves to the background. This is relevant to long cold
  builds.

### 1e. Claude Code sandbox: VERIFIED (source + measured)
- Settings: `sandbox.enabled`, `failIfUnavailable`, `autoAllowBashIfSandboxed` (default true),
  `excludedCommands`, `allowUnsandboxedCommands`, `filesystem.allowWrite/denyWrite/denyRead/allowRead`,
  `allowNestedUserNamespaces`, `enableWeakerNestedSandbox`.
  [settings-reference#sandbox](https://code.claude.com/docs/en/settings-reference#sandbox)
- Default writes are the cwd, `$TMPDIR` and added dirs. A linked worktree also gets the shared
  `.git` (but not `hooks/` or `config`). Hooks, MCP servers and LSP run **outside** the
  sandbox. [sandboxing](https://code.claude.com/docs/en/sandboxing)
- Linux enforcement (srt `linux-sandbox-utils.ts`, `vendor/seccomp-src/seccomp-unix-block.c`):
  bwrap `--unshare-user --unshare-pid --unshare-net`, then an `apply-seccomp` helper whose
  filter returns EPERM for `unshare`/`clone` with `CLONE_NEWUSER|CLONE_NEWNS`, `setns`,
  `mount`, `umount2`, `pivot_root`, `open_tree`, `move_mount` and the `fs*` calls (`clone3`
  gets ENOSYS), and which writes `user.max_user_namespaces=0`. Without the helper, bwrap gets
  `--disable-userns` (ENOSPC). The doc agrees: "`unshare -U true` prints
  `unshare: unshare failed: Operation not permitted`".
- The fix is `sandbox.allowNestedUserNamespaces: true`, added in **2.1.305** and honored only
  from `~/.claude/settings.json`, `--settings` or managed settings, with a "turns off a
  security boundary" warning. "A command that begins with `unshare` stays sandboxed whatever
  `excludedCommands` pattern you list."
  [sandboxing#chromium-or-rootless-podman](https://code.claude.com/docs/en/sandboxing#chromium-or-rootless-podman-fails-inside-the-sandbox-on-linux)
- **Measured, srt 0.0.81:** by default nested `unshare -Urm` fails with EPERM. With
  `allowNestedUserNamespaces: true` the full Ferry pattern (tmpfs + bind + overlay) works,
  but only when the overlay upper/work dirs sit under an absolute `allowWrite` entry.
- **Measured, Claude 2.1.280** (sandbox on, `allowUnsandboxedCommands:false`): nested userns
  + mount **succeeded**, because this version predates the block.
- **Measured, `excludedCommands` (2.1.280):** `["./fakeferry *"]` plus allow rule
  `Bash(./fakeferry *)` ran the wrapper outside the sandbox. Matching is on command text:
  every subcommand must match, and a `cd`, redirect or `$(...)` keeps the call sandboxed.
  [settings-reference#sandbox-excludedcommands](https://code.claude.com/docs/en/settings-reference#sandbox-excludedcommands)
- **Measured, the other way round:** srt run *inside* a Ferry-like namespace works. It can
  write a canonical overlay target mount only if that path is in `allowWrite`; the
  canonical bind of the worktree stayed read-only because it wasn't listed.

### 1f. Headless / Agent SDK: VERIFIED
- `claude -p --worktree` skips the trust check. `--setting-sources user,project,local`
  limits which files load. `--settings <file|json>` and `--agents <json>` are available.
  [headless](https://code.claude.com/docs/en/headless), [cli-reference](https://code.claude.com/docs/en/cli-reference)
- TS SDK options: `cwd`, `env` (replaces, does not merge),
  `spawnClaudeCodeProcess(options) => SpawnedProcess` ("run Claude Code in VMs, containers,
  or remote environments"), `pathToClaudeCodeExecutable`, `sandbox`, `settingSources`.
  Python SDK has `cwd`, `cli_path` and `env` (merged).
  [agent-sdk/typescript](https://code.claude.com/docs/en/agent-sdk/typescript),
  [agent-sdk/python](https://code.claude.com/docs/en/agent-sdk/python)
- Transcripts go to `~/.claude/projects/<cwd with non-alnum→'->/`.
  `CLAUDE_CODE_PROJECT_DIR_NAME` overrides the name only together with `CLAUDE_CONFIG_DIR`,
  and only from the launch env. [sessions](https://code.claude.com/docs/en/sessions),
  [env-vars](https://code.claude.com/docs/en/env-vars)
  **Consequence:** if every agent runs at the same canonical cwd (e.g. `/ferry/work`), all
  agents share one project dir, `--continue` history and auto-memory.
- **Measured, minor:** on 2.1.280, `-p --worktree` sessions with a hook-created worktree wrote
  their transcript under the *worktree* path's project dir. The doc says hook worktrees keep
  the transcript at the launch dir.

---

## 2. OpenAI Codex (CLI 0.162.0, app)

- **Worktrees, app.** Managed worktrees live in `$CODEX_HOME/worktrees` (configurable in
  Settings > Worktrees). They start in detached HEAD from the chosen branch, and uncommitted
  changes are applied. `.worktreeinclude` is supported, and an ignored `AGENTS.override.md` is
  copied automatically. The app keeps the 15 most recent.
  [environments/git-worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees)
- **Worktrees, CLI.** `codex --worktree` / `codex exec --worktree` ("Run the session in a new
  managed Git worktree"), per `codex --help`. Feature flag `worktrees` is `stable true` per
  `codex features list`. Not on the docs site; seen in `codex-rs/cli/tests/worktree.rs`.
  **Measured:** created `~/.codex/worktrees/<4hex>/<repo-name>` on a detached HEAD, and
  SessionStart got `cwd` = that path. **There is no WorktreeCreate equivalent, so Codex
  can't be told to use a Ferry worktree.** Use `codex --cd <ferry worktree>` instead.
- **Config.** `~/.codex/config.toml` plus `<repo>/.codex/config.toml` (project layers need
  trust). Keys: `sandbox_mode = read-only|workspace-write|danger-full-access`;
  `sandbox_workspace_write.writable_roots`, `.network_access`, `.exclude_tmpdir_env_var`,
  `.exclude_slash_tmp`; `approval_policy`; `default_permissions` (built-ins `:read-only`,
  `:workspace`, `:danger-full-access`, or custom `[permissions.<name>]`; don't combine with
  `sandbox_mode`); `shell_environment_policy.inherit = all|core|none`, `.filters`, `.set`
  (literal map). CLI flags: `-C/--cd`, `--add-dir` ("Additional directories that should be
  writable").
  [config-reference](https://learn.chatgpt.com/docs/config-file/config-reference)
- **Linux sandbox:** "`bwrap` plus `seccomp` by default"; Codex uses the first `bwrap` on
  PATH, else a bundled helper.
  [agent-approvals-security](https://learn.chatgpt.com/docs/agent-approvals-security),
  [sandboxing](https://learn.chatgpt.com/docs/sandboxing)
  - Source (`codex-rs/linux-sandbox/src/bwrap.rs`, `seccomp.rs` @ 970111f): `--unshare-user`,
    `--unshare-ipc`, `--unshare-pid`, optional `--unshare-net`, `--cap-drop ALL`, and **no
    `--disable-userns`**. Seccomp denies ptrace, process_vm_*, io_uring and the network
    syscalls, and **not unshare or mount**. Landlock is legacy-only.
  - **Measured:** `codex sandbox -P :workspace -C . -- unshare -Urm sh ferrylike.sh` mounted
    tmpfs + bind + overlay and wrote to the overlay. Under `-P :read-only` the overlay failed
    because the upper dir was read-only.
  - Protected paths: `<writable_root>/.git` is read-only, and so is the resolved gitdir of a
    `.git` pointer file. Commits from a worktree inside the sandbox therefore need escalation.
  - macOS uses Seatbelt via `sandbox-exec`, which is irrelevant to namespaces.
- **Hooks, stable.** Locations are `~/.codex/hooks.json`, `~/.codex/config.toml`
  `[hooks]`, and the same pair under `<repo>/.codex/`. Non-managed hooks need `/hooks` trust
  (or `--dangerously-bypass-hook-trust`).
  - Events: `SessionStart`, `SessionEnd`, `SubagentStart`, `SubagentStop`, `PreToolUse`,
    `PermissionRequest`, `PostToolUse`, `PreCompact`, `PostCompact`, `UserPromptSubmit`,
    `Stop`, `Interrupt`. Matchers are regexes.
  - PreToolUse rewrite: `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"..."}}}`.
    For Bash, `updatedInput` must include a string `command` and is accepted **only with
    `"allow"`**. `"ask"` is parsed but unsupported.
  - Hook commands run with the session cwd. There is **no env-file mechanism**.
  [hooks](https://learn.chatgpt.com/docs/hooks)
  - **Measured:** an inline `-c 'hooks.PreToolUse=[{matcher="^Bash$",hooks=[{type="command",command="…"}]}]'`
    rewrote `cargo --version` to `echo VIA-FERRY-CODEX: …`. Input keys: `cwd, hook_event_name,
    model, permission_mode, session_id, tool_input, tool_name, tool_use_id, transcript_path, turn_id`.
- **Rules.** `prefix_rule(pattern=[...], decision="allow"|"prompt"|"forbidden")` in
  `~/.codex/rules/*.rules`. `allow` means "Run the command outside the sandbox without
  prompting." [rules](https://learn.chatgpt.com/docs/agent-configuration/rules)
- **AGENTS.md.** Global `~/.codex/AGENTS.override.md` or `AGENTS.md`, then each directory
  from the git root down to the cwd (`AGENTS.override.md` before `AGENTS.md`), capped at
  32 KiB (`project_doc_max_bytes`).
  [agents-md](https://learn.chatgpt.com/docs/agent-configuration/agents-md)
- **Inside a wrapper. Measured:** `codex sandbox -P :workspace` works inside
  `unshare -Urm` and inside an own-uid userns. So `ferry run a1 -- codex --cd <wt>` with
  Codex's sandbox on is viable.

---

## 3. Other agents (verified facts only)

- **Cursor.** Local parallel-agent worktrees run setup from `.cursor/worktrees.json`
  (`setup-worktree-unix`, `setup-worktree-windows`, `setup-worktree`), with
  `$ROOT_WORKTREE_PATH` available; `cursor.worktreeMaxCount` defaults to 25
  ([worktrees](https://cursor.com/docs/configuration/worktrees)). Cloud agents "run in
  isolated VMs in the cloud" ([cloud-agent](https://cursor.com/docs/cloud-agent)).
- **Gemini CLI.** Experimental `--worktree`/`-w` (`"experimental": {"worktrees": true}`)
  creates `.gemini/worktrees/<name>` on branch `<name>`, never auto-deleted
  ([git-worktrees](https://geminicli.com/docs/cli/git-worktrees/)). Hooks include
  `SessionStart`, `BeforeTool` and `AfterTool`, with no documented command-rewrite field
  ([hooks](https://geminicli.com/docs/hooks/)).
- **OpenCode.** Plugin hooks `tool.execute.before` can mutate `output.args.command` for bash,
  and `shell.env` can set `output.env.*`. Plugins receive a `worktree` path.
  [plugins](https://opencode.ai/docs/plugins/)
- **Aider.** Works in the git repo of its cwd. `--subtree-only`, `--env-file`, `--test-cmd`
  and `--lint-cmd` exist; no worktree feature.
  [options](https://aider.chat/docs/config/options.html)
- **Amp.** `-x` is execute mode. `amp.permissions` can `delegate` each tool call to a helper
  program (`amp permissions add delegate --to <helper> '*'`). Whether a delegate can rewrite
  a command is not verified.
  [tool-level permissions](https://ampcode.com/news/tool-level-permissions)

All of these accept a cwd (launch dir) and inherit env, so "spawn worktree, cd, run"
plus a cargo PATH shim is the generic path.

---

## 4. Recommended integration recipes

Ferry-side subcommands and flags marked *(proposed)* don't exist yet. Every agent-side key is
copied from the verified schemas above.

### R0. Generic: works for every agent
```sh
ferry spawn a1 --from HEAD
cd "$(ferry env a1 --worktree)"           # (proposed) print host worktree path
ferry run a1 -- <agent>                   # whole agent inside Ferry's namespace
```
- Start the agent at the **host** worktree path, not the canonical path. The bind is
  additive, so both paths resolve inside the namespace.
- Put Ferry's `cargo` shim first on PATH. The shim maps `$PWD` to the canonical path and
  execs the real cargo with the per-agent `CARGO_TARGET_DIR`.
- Outside a namespace (pattern 2), the shim itself enters the namespace for the duration of
  the cargo call.

### R1. Claude Code, interactive session in Ferry's sandbox (recommended default)
```sh
ferry spawn a1 && ferry run a1 -- claude
```
- Map own uid → own uid, not root, because bypass mode is refused as root.
- Claude's own sandbox can stay on (measured with srt). If enabled, the canonical work and
  target paths must be writable. Add to `~/.claude/settings.json`:
  ```json
  { "sandbox": { "enabled": true,
      "filesystem": { "allowWrite": ["/ferry/work", "/ferry/target"] } } }
  ```
  (Paths are illustrative: use Ferry's actual canonical mount points; Cargo.lock writes
  hit the work path.)

### R2. Claude Code, WorktreeCreate/WorktreeRemove hooks
This covers `claude -w`, `EnterWorktree` and `isolation: worktree` subagents. Put it in
`~/.claude/settings.json` (uncommitted project settings don't follow into worktrees).
```json
{
  "hooks": {
    "WorktreeCreate": [ { "hooks": [ { "type": "command", "command": "ferry hook claude-worktree-create", "timeout": 60 } ] } ],
    "WorktreeRemove": [ { "hooks": [ { "type": "command", "command": "ferry hook claude-worktree-remove" } ] } ],
    "SessionStart":   [ { "hooks": [ { "type": "command", "command": "ferry hook claude-session-start" } ] } ]
  }
}
```
Equivalent shell for the *(proposed)* `ferry hook claude-worktree-create`:
```bash
in=$(cat); name=$(jq -r .name <<<"$in"); repo=$(jq -r .cwd <<<"$in")
cd "$repo" && ferry spawn "cc-$name" --from HEAD >&2   # all chatter to stderr
ferry env "cc-$name" --worktree                         # LAST stdout line = absolute path
```
- `ferry hook claude-worktree-remove`: read `worktree_path`. Exit non-zero, leaving the
  directory in place, if it has uncommitted or unpushed work. Otherwise `ferry rm` it and
  delete its branch.
- `ferry hook claude-session-start`: only
  `echo 'export PATH="<ferry-shim-dir>:$PATH"' >> "$CLAUDE_ENV_FILE"`. **Never** write
  `CARGO_TARGET_DIR` (measured leak into subagents).
- Subagents: add `isolation: worktree` to `.claude/agents/<x>.md`. Their worktrees arrive as
  `cc-agent-<hex>` and get no WorktreeRemove, so `ferry gc` must reap clean, unlocked,
  unused `cc-*` agents.
- If Claude's sandbox is on (≥2.1.305), the shim's `unshare` is blocked. Pick one of:
  - `"sandbox": {"allowNestedUserNamespaces": true, "filesystem": {"allowWrite": ["<ferry state dir>"]}}`
    (weakens Claude's sandbox; user scope only), or
  - `"sandbox": {"excludedCommands": ["cargo *"]}` (cargo runs unsandboxed through Ferry; a
    `cd x && cargo` call stays sandboxed and then fails), or
  - recipe R1.

### R3. Claude Code, PreToolUse fallback (no PATH control)
```json
{ "hooks": { "PreToolUse": [ { "matcher": "Bash",
    "hooks": [ { "type": "command", "if": "Bash(cargo *)", "command": "ferry hook claude-pretooluse" } ] } ] } }
```
Hook output (measured shape): `{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{<all original tool_input fields>,"command":"ferry run <agent-from-cwd> -- <orig>"}}}`.
Then add a `permissions.allow` rule `Bash(ferry run *)`. `if` is a handler field: it goes
inside the inner hook object, as above. The hook rewrites only commands that *start* with
`cargo` (`cd x && cargo test` matches `if`, because each subcommand is checked, but a naive
prefix rewrite would then be wrong). The PATH shim avoids this whole class of problem.

### R4. Codex
- Launch either way:
  - `ferry run a1 -- codex --cd "$(ferry env a1 --worktree)"` (Codex's sandbox works inside), or
  - on the host, with `--add-dir <ferry state dir>` so overlay upper dirs are writable.
- `~/.codex/config.toml`:
  ```toml
  sandbox_mode = "workspace-write"
  [sandbox_workspace_write]
  writable_roots = ["/home/me/.local/share/ferry"]   # Ferry state/upper dirs (illustrative)

  [[hooks.PreToolUse]]
  matcher = "^Bash$"
  [[hooks.PreToolUse.hooks]]
  type = "command"
  command = "ferry hook codex-pretooluse"   # (proposed) returns permissionDecision "allow" + updatedInput.command
  ```
- PATH: Codex has no env file, and `shell_environment_policy.set` is literal. Prefer
  `ferry run … -- codex`, which puts the shim on PATH, or a launcher that exports PATH before
  `codex`.
- Don't rely on `codex --worktree`. It picks its own path under `$CODEX_HOME/worktrees` and
  has no creation hook.

---

## Design implications for Ferry

1. **Resolve the agent from `$PWD`, never from an inherited env var.** Claude propagates
   session env (the `CLAUDE_ENV_FILE` content) into worktree-isolated subagents, so
   `FERRY_AGENT`/`CARGO_TARGET_DIR` exports would cross-wire agents. The shim should run
   `git rev-parse --show-toplevel`, look the path up in Ferry's registry, and refuse to run
   if nothing is found. Do not fall back to the main target.
2. **Ship a `cargo` (and `rustc`?) PATH shim as the primary interception.** Make it work both
   inside a Ferry namespace (just remap cwd and exec) and outside it (unshare, mount, remap,
   exec). Detect "already inside" by comparing the canonical mount's dev/ino with the
   worktree root, not with an env var. Hook rewrites are secondary.
3. **Keep host paths valid inside the namespace and start agents at the host path.** The
   canonical path is only for cargo/rustc. This avoids transcript and memory collisions
   (`~/.claude/projects/<cwd>`) and keeps Claude's `pwd -P` cwd tracking sane.
4. **Map own uid in the userns** (verified with direct `mount(2)`), not root, and call
   mount(2) directly. Root mapping breaks Claude's bypass mode, and util-linux `mount`
   refuses under own-uid mapping.
5. **Make `ferry spawn` hook-friendly.** It must be quick (default hook timeout is 600 s, but
   users will wait on it), print nothing but the path on stdout (offer a `--print-path` /
   `ferry env --worktree` style), take its base from the caller's `HEAD` (the hook input
   carries no base ref), and fall back to the nearest base rather than building one inside a
   hook. Optionally honor `.worktreeinclude`, since Claude skips it for hook worktrees.
6. **`ferry rm` must be safe as a WorktreeRemove target.** Refuse when the tree is dirty or
   has unpushed commits (exit non-zero, leave the dir), and delete the branch Ferry created.
7. **`ferry gc` must reap orphaned agent worktrees**: Claude subagent/workflow worktrees and
   all `-p` sessions never trigger WorktreeRemove. Respect `git worktree lock` (Claude holds
   it while an agent runs).
8. **Detect a blocked nested userns and say what to do.** EPERM comes from the srt seccomp
   filter, ENOSPC from `--disable-userns`/max_user_namespaces=0. The message should name
   `sandbox.allowNestedUserNamespaces`, `excludedCommands`, or `ferry run -- claude`.
9. **Keep all writable Ferry state under one absolute directory**, so a single `allowWrite` /
   `writable_roots` / `--add-dir` entry covers overlay upper/work dirs. Use fresh upper/work
   pairs per mount: reusing a stale pair made an overlay come up read-only in one run.
10. **Document the foot-guns:** never add `target` to Claude's `worktree.symlinkDirectories`
    or any other cross-worktree sharing, and never put a shared `CARGO_TARGET_DIR` in
    settings `env`.
11. Optionally write a short gitignored `AGENTS.override.md` / `CLAUDE.local.md` into each
    Ferry worktree explaining `ferry run`. Codex reads `AGENTS.override.md` per directory.

## Open questions / not verified

- On Claude ≥2.1.305 the nested-userns block is verified only via srt 0.0.81 run standalone
  and the docs. I did not run Claude 2.1.310 itself; the local binary is 2.1.280.
- Does Codex's sandbox still apply to a command rewritten by PreToolUse `"allow"`? The
  measured run used `exec` with workspace-write, but I did not test a sandbox-denied write
  after a rewrite.
- Does rustc's incremental reuse need the *target dir* at a canonical path, or only the
  source path? If only the source, the shim could leave `CARGO_TARGET_DIR` at a per-agent
  host path. That makes test-binary paths printed by `cargo test` valid on the host, and
  removes the need to allowlist a canonical target path in sandboxes.
- Agents may try to open the canonical paths that cargo prints (e.g. `Running …/ferry/target/debug/deps/x-hash`)
  from the host, where they don't exist. Can Ferry make those resolve on the host?
- Does Codex inherit project trust for linked worktrees (`core/src/worktree_trust*`)? Not
  confirmed.
- Claude `PreToolUse` `updatedInput` without `permissionDecision` worked here (measured) but is
  not explicitly documented as supported. Prefer `"allow"` paired with a matching allow rule.
- Amp delegate semantics, Cursor's worktree root path, and OpenCode env propagation into
  subprocesses were not verified beyond the linked pages.

Leftovers from probes: transcripts under `~/.claude/projects/*research-agents-ccprobe*`.
Deleting them was denied by the permission system, so remove them manually if wanted.
