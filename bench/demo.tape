# Run through bench/demo.sh, which prepares $DEMO_WORK and the warm base.
Output docs/src/demo.gif

Set Shell "bash"
Set FontSize 20
Set Width 1000
Set Height 640
Set Padding 18
Set Theme "Catppuccin Mocha"
Set TypingSpeed 35ms
Set WaitPattern /\$\s*$/
Set WaitTimeout 120s

# `time` prints and records each step's wall time; `summary` compares them.
Hide
Type `cd "$DEMO_WORK/ripgrep" && export PS1='$ ' && TIMES=()`
Enter
Wait
Type `t() { local s=$EPOCHREALTIME rc; "$@"; rc=$?; TIMES+=("$(awk "BEGIN { printf \"%.2f\", $EPOCHREALTIME - $s }")"); echo "⏱  ${TIMES[-1]}s"; return $rc; }; alias time='t '`
Enter
Wait
Type `summary() { clear; printf '\n  \e[1mripgrep, first build in a new worktree\e[0m\n\n    git worktree     %6.2fs\n    \e[1;32mferry agent      %6.2fs   %s× faster\e[0m\n\n  ferry spawn        %6.2fs\n  first edit         %6.2fs   incremental\n\n' "${TIMES[0]}" "${TIMES[2]}" "$(awk "BEGIN { printf \"%.0f\", ${TIMES[0]} / ${TIMES[2]} }")" "${TIMES[1]}" "${TIMES[3]}"; }; clear`
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
