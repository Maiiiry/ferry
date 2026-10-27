# Run through bench/demo.sh, which prepares $DEMO_WORK and the warm base.
Output docs/src/demo.gif

Set Shell "bash"
Set FontSize 15
Set Width 1400
Set Height 820
Set Padding 18
Set Theme "Catppuccin Mocha"
Set TypingSpeed 35ms
Set WaitPattern /\$\s*$/
Set WaitTimeout 120s

# `time` prints and records each step's wall time; `summary` lists them.
Hide
Type `cd "$DEMO_WORK/ripgrep" && export PS1='$ ' && TIMES=()`
Enter
Wait
Type `t() { local s=$EPOCHREALTIME rc; "$@"; rc=$?; TIMES+=("$(awk "BEGIN { printf \"%.2fs\", $EPOCHREALTIME - $s }")"); echo "⏱  ${TIMES[-1]}"; return $rc; }; alias time='t '`
Enter
Wait
Type `summary() { echo; printf '  %-34s %s\n' "new git worktree, first build" "${TIMES[0]}" "ferry spawn" "${TIMES[1]}" "ferry agent, first build" "${TIMES[2]}" "ferry agent, first edit + build" "${TIMES[3]}"; }; clear`
Enter
Wait
Show

Type "# ripgrep. Each agent needs its own worktree."
Enter
Type "# Without Ferry, a new worktree starts cold:"
Enter
Type "git worktree add -q ../plain-agent && cd ../plain-agent"
Enter
Wait
Type "time cargo build"
Enter
Wait
Sleep 3s
Type "cd ../ripgrep && clear"
Enter
Wait

Type "# With Ferry, the agent forks a warm base (built once by `ferry base`):"
Enter
Type "time ferry spawn agent"
Enter
Wait
Type `eval "$(ferry env agent)" && cd "$FERRY_WORKTREE"`
Enter
Wait
Type "time cargo build"
Enter
Wait
Sleep 1s
Type "# The agent edits a crate: only it and its dependents rebuild, incrementally."
Enter
Type `printf '/// Demo.\npub fn demo() {}\n' >> crates/ignore/src/lib.rs`
Enter
Wait
Type "time cargo build"
Enter
Wait
Sleep 1s
Type "summary"
Enter
Wait
Sleep 6s
