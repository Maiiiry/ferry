# Part one of the demo, run by bench/demo.sh. It is played back 10x faster,
# so typing and pauses here are 10x slower than they will look.
Output target/demo-cold.mp4

Set Shell "bash"
Set FontSize 20
Set Width 1000
Set Height 640
Set Padding 18
Set Theme "Catppuccin Mocha"
Set TypingSpeed 350ms
Set WaitPattern /\$\s*$/
Set WaitTimeout 600s

# `time` prints the wall time and saves it for part two's summary.
Hide
Type `cargo() { command cargo "$@" --color always 2>&1 | sed -u -E 's/ \(\/[^)]*\)//' | grep --line-buffered -E 'Compiling|Finished|error'; return "${PIPESTATUS[0]}"; }`
Enter
Wait
Type `cd "$DEMO_WORK/rust-analyzer" && export PS1='$ '`
Enter
Wait
Type `t() { local s=$EPOCHREALTIME rc; "$@"; rc=$?; local d; d=$(awk "BEGIN { printf \"%.2f\", $EPOCHREALTIME - $s }"); echo "$d" >>"$DEMO_WORK/times"; echo "⏱  ${d}s"; return $rc; }; alias time='t '; clear`
Enter
Wait
Show

Type "# rust-analyzer: 281 crates. Each agent needs its own worktree."
Enter
Type "# Without Ferry, a new worktree starts cold (played 10x faster):"
Enter
Type "git worktree add -q ../plain-agent && cd ../plain-agent"
Enter
Wait
Type "time cargo build"
Enter
Wait
Sleep 30s
