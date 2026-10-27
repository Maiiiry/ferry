# Part two of the demo, run by bench/demo.sh after part one, at real speed.
Output target/demo-ferry.mp4

Set Shell "bash"
Set FontSize 20
Set Width 1000
Set Height 640
Set Padding 18
Set Theme "Catppuccin Mocha"
Set TypingSpeed 35ms
Set WaitPattern /\$\s*$/
Set WaitTimeout 300s

# `time` prints and saves each step's wall time; `summary` compares them with
# part one's cold build.
Hide
Type `cd "$DEMO_WORK/rust-analyzer" && export PS1='$ ' && mapfile -t TIMES <"$DEMO_WORK/times"`
Enter
Wait
Type `t() { local s=$EPOCHREALTIME rc; "$@"; rc=$?; TIMES+=("$(awk "BEGIN { printf \"%.2f\", $EPOCHREALTIME - $s }")"); echo "⏱  ${TIMES[-1]}s"; return $rc; }; alias time='t '`
Enter
Wait
Type `summary() { clear; printf '\n  \e[1mrust-analyzer, first build in a new worktree\e[0m\n\n    git worktree     %7.2fs\n    \e[1;32mferry agent      %7.2fs   %s× faster\e[0m\n\n  ferry spawn        %7.2fs\n  first edit         %7.2fs   incremental\n\n' "${TIMES[0]}" "${TIMES[2]}" "$(awk "BEGIN { printf \"%.0f\", ${TIMES[0]} / ${TIMES[2]} }")" "${TIMES[1]}" "${TIMES[3]}"; }; clear`
Enter
Wait
Show

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
Type `printf '/// Demo.\npub fn demo() {}\n' >> crates/ide-db/src/lib.rs`
Enter
Wait
Type "time cargo build"
Enter
Wait
Sleep 1s
Type "summary"
Enter
Wait
Sleep 7s
