#!/usr/bin/env bash
# Records docs/src/demo.gif on rust-analyzer. Part one is a new git worktree's
# cold build, recorded in real time and played back 10x faster (it says so on
# screen). Part two is a Ferry agent's spawn, first build and first edit at
# real speed, ending on the measured times of both parts. Needs vhs (with ttyd
# and a Chromium), ffmpeg, git, cargo and a ferry binary (default: this
# checkout's release build).
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# Short paths keep the recorded output readable.
work=${DEMO_WORK:-$HOME/.cache/ferry-demo}
ferry=${FERRY:-$root/target/release/ferry}
rev=0c4d74686b21f2b3bd8872b6d3cf36ad2e740d49
speedup=10

mkdir -p "$work/bin"
ln -sf "$ferry" "$work/bin/ferry"
[[ -d $work/rust-analyzer ]] || git clone -q https://github.com/rust-lang/rust-analyzer "$work/rust-analyzer"

export FERRY_HOME=$work/home FERRY_CONFIG=$work/ferry.toml PATH=$work/bin:$PATH DEMO_WORK=$work
printf '[base]\ntargets = ["build"]\n' >"$FERRY_CONFIG"
cd "$work/rust-analyzer"
ferry rm --force agent >/dev/null 2>&1 || true
git worktree remove --force ../plain-agent >/dev/null 2>&1 || true
git checkout -q -B main "$rev"
# Downloads happen here, not on camera.
cargo fetch -q
ferry base
rm -f "$work/times"

cd "$root"
mkdir -p target
vhs bench/demo-cold.tape
vhs bench/demo-ferry.tape
# Both parts share one canvas, so they join without a seam.
ffmpeg -v error -y -i target/demo-cold.mp4 -i target/demo-ferry.mp4 -filter_complex \
  "[0:v]setpts=PTS/$speedup[a];[a][1:v]concat=n=2:v=1[v];[v]fps=12,split[x][y];[x]palettegen=stats_mode=diff[p];[y][p]paletteuse=dither=bayer:bayer_scale=4" \
  docs/src/demo.gif
