#!/bin/sh
# Starts Codex in a Ferry agent: creates the agent if it does not exist, then
# runs Codex inside the agent's build context, in the agent's worktree.
#
# Usage (inside the repository, after `ferry base`):
#   ferry-codex.sh <agent-name> [codex arguments...]
#
# Examples:
#   ferry-codex.sh fix-parser
#   ferry-codex.sh fix-parser exec "make the parser tests pass"
#
# Every cargo command Codex runs then builds in the agent's own warm target
# dir. Nothing exports CARGO_TARGET_DIR: Ferry's cargo shim, first on PATH
# inside `ferry run`, picks the target dir from the working directory.
set -eu

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <agent-name> [codex arguments...]" >&2
    exit 2
fi
name=$1
shift

# `ferry env` fails when the agent does not exist yet.
if ! ferry env "$name" >/dev/null 2>&1; then
    ferry spawn "$name" >&2
fi

# `ferry env` prints shell-quoted `export` lines; read the worktree path from
# them in a subshell instead of parsing JSON.
worktree=$(eval "$(ferry env "$name")" && printf '%s' "$FERRY_WORKTREE")

exec ferry run "$name" -- codex --cd "$worktree" "$@"
