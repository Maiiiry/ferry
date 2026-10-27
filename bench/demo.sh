#!/usr/bin/env bash
# Records docs/src/demo.gif: a new git worktree building ripgrep cold, next to
# a Ferry agent forked from a warm base. Needs vhs (with ttyd, ffmpeg and a
# Chromium), git, cargo and a ferry binary (default: this checkout's release
# build). The work dir defaults to /tmp/ferry-demo.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# Short paths keep the recorded output readable.
work=${DEMO_WORK:-/tmp/ferry-demo}
ferry=${FERRY:-$root/target/release/ferry}
rev=6892bc2b7efe5909cbbeec8f757ed2c4c76b1d30

mkdir -p "$work/bin"
ln -sf "$ferry" "$work/bin/ferry"
[[ -d $work/ripgrep ]] || git clone -q https://github.com/BurntSushi/ripgrep "$work/ripgrep"

export FERRY_HOME=$work/home PATH=$work/bin:$PATH DEMO_WORK=$work
cd "$work/ripgrep"
ferry rm --force agent >/dev/null 2>&1 || true
git worktree remove --force ../plain-agent >/dev/null 2>&1 || true
git checkout -q -B main "$rev"
# Downloads happen here, not on camera.
cargo fetch -q
ferry base
cd "$root"
vhs bench/demo.tape
