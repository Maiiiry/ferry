#!/usr/bin/env bash
# Functions shared by bench/run.sh and by the commands hyperfine runs.
#
# hyperfine calls these as `bash bench/lib.sh <function> [args]` (prepare,
# conclude and some timed commands), so all state comes from the B_* variables
# that run.sh exports:
#
#   B_ROOT B_LIB B_WORK B_REPO B_REV B_EDIT B_LIB_SRC B_BUILD_ARGS B_FERRY
#   B_LOGS B_OUT B_STATE B_AGENTS B_KACHE
#
# Per-run files are named $B_LOGS/<scenario>-<step>.<iteration>.<kind>, where
# <iteration> is hyperfine's $HYPERFINE_ITERATION (0-based).
set -euo pipefail
# $B_BUILD_ARGS is split into words on purpose: it holds extra cargo arguments.
# shellcheck disable=SC2086

die() {
  echo "bench: error: $*" >&2
  exit 1
}
note() { echo "bench: $*" >&2; }

# --- git worktrees -----------------------------------------------------------

wt_remove() { # <path>
  if [[ -e $1 ]]; then
    git -C "$B_REPO" worktree remove --force --force "$1" 2>/dev/null || {
      chmod -R u+rwx "$1" 2>/dev/null || true
      rm -rf "$1"
    }
  fi
  git -C "$B_REPO" worktree prune
}

# A fresh checkout of B_REV: every file gets a new mtime, like a new agent's.
wt_new() { # <path>
  wt_remove "$1"
  git -C "$B_REPO" worktree add --quiet --detach "$1" "$B_REV"
}

# --- edits and markers -------------------------------------------------------

label() { # <scenario> <step> <run> [suffix]: unique per run, so content caches never hit
  local l="${1}_${2}_r${3}${4:+_$4}"
  l=${l//[^a-zA-Z0-9_]/_}
  echo "${l,,}"
}

marker() { # <label>
  echo "FERRY_MARK[$1]"
}

# Appends a documented pub fn whose string literal ends up in the crate's rlib.
edit() { # <checkout> <label>
  [[ -f $1/$B_EDIT ]] || die "edit target $1/$B_EDIT does not exist"
  printf '\n/// Benchmark edit `%s`, appended by bench/run.sh.\npub fn ferry_bench_%s() -> &'\''static str {\n    "%s"\n}\n' \
    "$2" "$2" "$(marker "$2")" >>"$1/$B_EDIT"
}

# --- cargo output ------------------------------------------------------------

# Units cargo compiled (compiler-artifact with fresh:false), as "name (kind)".
# Lines that are not JSON (ferry's own stderr output) are skipped.
crate_list() { # <log>
  jq -R -s -c '[split("\n")[] | fromjson? | select(.reason == "compiler-artifact" and .fresh == false)
    | .target.name + " (" + (.target.kind | join(",")) + ")"]' "$1"
}

# The edited package's rlib, as cargo reported it (host or canonical path).
rlib_of() { # <log>
  jq -R -r --arg src "/$B_LIB_SRC" 'fromjson? | select(.reason == "compiler-artifact"
      and (.target.src_path | endswith($src)) and (.target.kind | any(. == "lib" or . == "rlib")))
    | .filenames[] | select(endswith(".rlib"))' "$1" | tail -n 1
}

# Every benchmark marker compiled into the edited crate, compared with the
# markers this agent's own edits should have put there.
verify() { # <log> <agent or -> [expected labels...]
  local log=$1 agent=$2 rlib found expected
  shift 2
  rlib=$(rlib_of "$log")
  [[ -n $rlib ]] || die "no rlib for $B_LIB_SRC in $log"
  if [[ $agent == - ]]; then
    found=$(grep -a -o -E 'FERRY_MARK\[[a-z0-9_]+\]' "$rlib" | sort -u || true)
  else
    # Overlay targets are only visible inside the agent's sandbox.
    found=$(fq run "$agent" -- grep -a -o -E 'FERRY_MARK\[[a-z0-9_]+\]' "$rlib" | sort -u || true)
  fi
  expected=$(for l in "$@"; do marker "$l"; done | sort -u)
  jq -n -c --arg rlib "$rlib" --arg found "$found" --arg expected "$expected" '
    ($found | split("\n") | map(select(. != ""))) as $f
    | ($expected | split("\n") | map(select(. != ""))) as $e
    | {rlib: $rlib, found: $f, expected: $e, ok: ($f == $e)}'
}

loadavg() { cut -d' ' -f1-3 /proc/loadavg; }

# Allocated bytes (du) and, on btrfs, bytes not shared with any other file.
disk_json() { # <dir>
  local du total excl shared
  du=$(du -s -B1 "$1" 2>/dev/null | cut -f1 || true)
  read -r total excl shared _ < <(btrfs filesystem du -s --raw "$1" 2>/dev/null | tail -n 1 || true) || true
  [[ $total =~ ^[0-9]+$ ]] || total='' excl='' shared=''
  jq -n -c --arg path "$1" --arg du "${du:-}" --arg t "$total" --arg e "$excl" --arg s "$shared" '
    def num: if . == "" then null else tonumber end;
    {path: $path, du_bytes: ($du | num), btrfs_total_bytes: ($t | num),
     btrfs_exclusive_bytes: ($e | num), btrfs_shared_bytes: ($s | num)}'
}

# --- ferry -------------------------------------------------------------------

fq() { "$B_FERRY" -C "$B_REPO" "$@"; }

ferry_has_agent() { fq list --json | jq -e --arg n "$1" 'any(.agents[]; .name == $n)' >/dev/null; }

ferry_rm() { # <agent>...
  local a
  for a in "$@"; do
    if ferry_has_agent "$a"; then fq rm --force "$a" >/dev/null 2>>"$B_LOGS/ferry-rm.log"; fi
  done
}

ferry_wt() { fq env "$1" --json | jq -r .worktree; }

ferry_store() { # the per-repository store under FERRY_HOME
  local s
  for s in "$FERRY_HOME"/*/store.json; do
    [[ -f $s ]] && dirname "$s" && return
  done
  die "no ferry store under $FERRY_HOME"
}

# Removes every agent, then every base (or only the bases at <commit>).
ferry_drop_bases() { # [commit]
  local a d
  for a in $(fq list --json | jq -r '.agents[].name'); do fq rm --force "$a" >/dev/null 2>>"$B_LOGS/ferry-rm.log"; done
  for d in "$FERRY_HOME"/*/bases/"${1:-}"*/; do
    [[ -d $d ]] || continue
    d=${d%/}
    git -C "$B_REPO" worktree unlock "$d/src" 2>/dev/null || true
    git -C "$B_REPO" worktree remove --force --force "$d/src" 2>/dev/null || true
    chmod -R u+rwx "$d" 2>/dev/null || true
    rm -rf "$d" "$d.lock"
  done
  git -C "$B_REPO" worktree prune
}

ferry_agent_state_dir() { # <agent>: the overlay upper dir, or the forked target dir
  local d
  d="$(ferry_store)/agents/$1"
  if [[ -d $d/upper ]]; then echo "$d/upper"; else echo "$d/target"; fi
}

# --- scenario agents -----------------------------------------------------------

agent_dir() { # <scenario>: the checkout the scenario's agent builds in
  case $1 in
    cold | naive | kache) echo "$B_WORK/wt/$1" ;;
    ferry-*) ferry_wt bench ;;
    *) die "agent_dir: unknown scenario $1" ;;
  esac
}

# A new agent with no edits: a new checkout plus the scenario's target.
fresh() { # <scenario>
  case $1 in
    cold | kache) wt_new "$B_WORK/wt/$1" ;;
    naive)
      wt_new "$B_WORK/wt/naive"
      # GNU cp 9+ reflinks by default where it can (btrfs, XFS).
      cp -a "$B_WORK/wt/naive-donor/target" "$B_WORK/wt/naive/target"
      ;;
    ferry-*)
      ferry_rm bench
      fq spawn bench >/dev/null
      ;;
    *) die "fresh: unknown scenario $1" ;;
  esac
}

# An untimed build, logged like a timed one.
build() { # <scenario> <log prefix>
  local rc=0
  case $1 in
    ferry-*) fq run bench -- cargo build $B_BUILD_ARGS --message-format=json >"$2.json" 2>"$2.err" || rc=$? ;;
    *) (cd "$(agent_dir "$1")" && exec cargo build $B_BUILD_ARGS --message-format=json) >"$2.json" 2>"$2.err" || rc=$? ;;
  esac
  if ((rc != 0)); then
    tail -n 20 "$2.err" >&2
    die "build failed ($1), see $2.err"
  fi
}

# Labels a step's edits; second_edit makes two (a, then b).
step_labels() { # <scenario> <step> <run>
  case $2 in
    spawn_build) ;;
    edit_on_spawn | first_edit) label "$1" "$2" "$3" ;;
    second_edit) echo "$(label "$1" "$2" "$3" a) $(label "$1" "$2" "$3" b)" ;;
    *) die "step_labels: unknown step $2" ;;
  esac
}

# hyperfine --prepare for the build steps: brings a new agent to the state
# just before the timed build.
prep() { # <scenario> <step>
  local sc=$1 step=$2 i=${HYPERFINE_ITERATION:?} p dir labels
  p="$B_LOGS/$sc-$step.$i"
  fresh "$sc"
  dir=$(agent_dir "$sc")
  read -r -a labels <<<"$(step_labels "$sc" "$step" "$i")"
  case $step in
    spawn_build) ;;
    edit_on_spawn) edit "$dir" "${labels[0]}" ;;
    first_edit)
      build "$sc" "$p.pre1"
      edit "$dir" "${labels[0]}"
      ;;
    second_edit)
      build "$sc" "$p.pre1"
      edit "$dir" "${labels[0]}"
      build "$sc" "$p.pre2"
      edit "$dir" "${labels[1]}"
      ;;
  esac
  if [[ $sc == kache ]]; then kache_events_count >"$p.kev"; fi
}

# hyperfine --conclude for the build steps: checks the artifact and records
# kache's per-crate results.
conclude() { # <scenario> <step>
  local sc=$1 step=$2 i=${HYPERFINE_ITERATION:?} p agent=- labels
  p="$B_LOGS/$sc-$step.$i"
  read -r -a labels <<<"$(step_labels "$sc" "$step" "$i")"
  [[ $sc == ferry-* ]] && agent=bench
  verify "$p.json" "$agent" "${labels[@]}" >"$p.verify.json"
  if [[ $sc == kache ]]; then kache_events_since "$(<"$p.kev")" >"$p.kache.json"; fi
}

# The timed build: a single cargo invocation, output into per-run files.
timed_build_cmd() { # <scenario> <step>
  local out="$B_LOGS/$1-$2" cmd
  case $1 in
    ferry-*) cmd="exec $(printf %q "$B_FERRY") -C $(printf %q "$B_REPO") run bench -- cargo build $B_BUILD_ARGS --message-format=json" ;;
    *) cmd="cd $(printf %q "$B_WORK/wt/$1") && exec cargo build $B_BUILD_ARGS --message-format=json" ;;
  esac
  echo "$cmd >$(printf %q "$out").\$HYPERFINE_ITERATION.json 2>$(printf %q "$out").\$HYPERFINE_ITERATION.err"
}

# --- kache ----------------------------------------------------------------------

kache_events_count() {
  if [[ -f $KACHE_RUNTIME_DIR/events.jsonl ]]; then wc -l <"$KACHE_RUNTIME_DIR/events.jsonl"; else echo 0; fi
}

kache_events_since() { # <line count before the build>
  { tail -n +"$(($1 + 1))" "$KACHE_RUNTIME_DIR/events.jsonl" 2>/dev/null || true; } | jq -R -s -c '
    [split("\n")[] | fromjson?] as $ev
    | {events: ($ev | length),
       by_result: ($ev | group_by(.result // "unknown") | map({key: (.[0].result // "unknown"), value: length}) | from_entries),
       compiled: [$ev[] | select(.result != "local_hit" and .result != "remote_hit" and ((.passthrough_reason // "") | startswith("not-a-compile") | not)) | {crate: .crate_name, result, reason: .passthrough_reason, ms: .elapsed_ms}]}'
}

# --- concurrent builds ------------------------------------------------------------

# Prints "<rc> <start> <end>" for one build into <prefix>.time.
_timed_bg() { # <prefix> <command...>
  local p=$1 s rc=0
  shift
  s=$(date +%s.%N)
  ("$@") >"$p.json" 2>"$p.err" || rc=$?
  echo "$rc $s $(date +%s.%N)" >"$p.time"
}

_wait_all() { # <prefix of per-agent files>
  local a rc
  wait
  for a in $(seq 1 "$B_AGENTS"); do
    read -r rc _ <"$1.a$a.time"
    ((rc == 0)) || {
      tail -n 20 "$1.a$a.err" >&2
      die "concurrent build $a failed, see $1.a$a.err"
    }
  done
}

_shared_cargo() { # <worktree>
  cd "$1" && CARGO_TARGET_DIR="$B_WORK/shared-target" exec cargo build $B_BUILD_ARGS --message-format=json
}

# Per-agent summary of a concurrent run: elapsed, crates, lock waits, check.
_agents_json() { # <prefix> <agent kind: shared|ferry> <scenario>
  local p=$1 a rc s e v
  for a in $(seq 1 "$B_AGENTS"); do
    read -r rc s e <"$p.a$a.time"
    if [[ $2 == ferry ]]; then
      v=$(verify "$p.a$a.json" "c$a" "$(label "$3" concurrent "${HYPERFINE_ITERATION:?}" "a$a")")
    else
      v=null
    fi
    jq -n -c --arg a "$a" --argjson rc "$rc" --arg s "$s" --arg e "$e" --argjson crates "$(crate_list "$p.a$a.json")" \
      --argjson bd "$(grep -c 'Blocking waiting for file lock on build directory' "$p.a$a.err" || true)" \
      --argjson pc "$(grep -c 'Blocking waiting for file lock on package cache' "$p.a$a.err" || true)" \
      --argjson verify "$v" '
      {agent: ($a | tonumber), rc: $rc, start: ($s | tonumber), end: ($e | tonumber),
       elapsed: (($e | tonumber) - ($s | tonumber)), crates: ($crates | length), crate_list: $crates,
       build_dir_lock_waits: $bd, package_cache_lock_waits: $pc, verify: $verify}'
  done | jq -s -c .
}

shared_setup() {
  local a
  rm -rf "$B_WORK/shared-target" "$B_WORK/shared-snap"
  for a in $(seq 1 "$B_AGENTS"); do wt_new "$B_WORK/wt/shared-$a"; done
  # Build once from every worktree so the target is fresh for all of them.
  for a in $(seq 1 "$B_AGENTS"); do
    (_shared_cargo "$B_WORK/wt/shared-$a") >"$B_LOGS/shared-setup.a$a.json" 2>"$B_LOGS/shared-setup.a$a.err" ||
      die "shared setup build failed, see $B_LOGS/shared-setup.a$a.err"
  done
  cp -a "$B_WORK/shared-target" "$B_WORK/shared-snap"
}

shared_reset() {
  local a
  rm -rf "$B_WORK/shared-target"
  cp -a "$B_WORK/shared-snap" "$B_WORK/shared-target"
  for a in $(seq 1 "$B_AGENTS"); do git -C "$B_WORK/wt/shared-$a" checkout --quiet -- .; done
}

shared_conc_prep() {
  local a
  shared_reset
  for a in $(seq 1 "$B_AGENTS"); do
    edit "$B_WORK/wt/shared-$a" "$(label shared concurrent "${HYPERFINE_ITERATION:?}" "a$a")"
  done
}

shared_conc_run() {
  local p="$B_LOGS/shared-concurrent.${HYPERFINE_ITERATION:?}" a
  for a in $(seq 1 "$B_AGENTS"); do
    _timed_bg "$p.a$a" _shared_cargo "$B_WORK/wt/shared-$a" &
  done
  _wait_all "$p"
}

shared_conc_conclude() {
  local p="$B_LOGS/shared-concurrent.${HYPERFINE_ITERATION:?}"
  _agents_json "$p" shared shared >"$p.agents.json"
}

# B edits, A edits differently and builds, then B builds. Single shot.
shared_contamination() {
  local a="$B_WORK/wt/shared-1" b="$B_WORK/wt/shared-2" p="$B_LOGS/shared-contamination" la lb
  la=$(label shared contamination 0 a)
  lb=$(label shared contamination 0 b)
  shared_reset
  edit "$b" "$lb"
  edit "$a" "$la"
  (_shared_cargo "$a") >"$p.a.json" 2>"$p.a.err" || die "contamination build A failed"
  (_shared_cargo "$b") >"$p.b.json" 2>"$p.b.err" || die "contamination build B failed"
  jq -n -c --argjson a "$(crate_list "$p.a.json")" --argjson b "$(crate_list "$p.b.json")" \
    --argjson v "$(verify "$p.b.json" - "$lb")" --arg ma "$(marker "$la")" --arg mb "$(marker "$lb")" '
    {a_crates: ($a | length), a_crate_list: $a, b_crates: ($b | length), b_crate_list: $b,
     b_rlib: $v.rlib, b_rlib_markers: $v.found,
     b_has_own_edit: ($v.found | index($mb) != null), b_has_a_edit: ($v.found | index($ma) != null)}'
}

ferry_conc_prep() {
  local a i=${HYPERFINE_ITERATION:?}
  ferry_rm bench $(seq -f 'c%g' 1 "$B_AGENTS")
  for a in $(seq 1 "$B_AGENTS"); do
    fq spawn "c$a" >/dev/null
    edit "$(ferry_wt "c$a")" "$(label "$1" concurrent "$i" "a$a")"
  done
}

ferry_conc_run() {
  local p="$B_LOGS/$1-concurrent.${HYPERFINE_ITERATION:?}" a
  for a in $(seq 1 "$B_AGENTS"); do
    _timed_bg "$p.a$a" fq run "c$a" -- cargo build $B_BUILD_ARGS --message-format=json &
  done
  _wait_all "$p"
}

ferry_conc_conclude() {
  local p="$B_LOGS/$1-concurrent.${HYPERFINE_ITERATION:?}"
  _agents_json "$p" ferry "$1" >"$p.agents.json"
}

# Checks that a base built from the committed edit contains it. The log has
# canonical paths; the base's target dir is the host side of them.
verify_base() { # <log> <commit> <label>
  local store rlib host base
  store=$(ferry_store)
  rlib=$(rlib_of "$1")
  [[ -n $rlib ]] || die "no rlib for $B_LIB_SRC in $1"
  base=$(ls -d "$store/bases/$2"*/ | head -n 1)
  host="${base%/}/target/${rlib#"$store/mnt/target/"}"
  local found
  found=$(grep -a -o -E 'FERRY_MARK\[[a-z0-9_]+\]' "$host" | sort -u || true)
  jq -n -c --arg rlib "$host" --arg found "$found" --arg expected "$(marker "$3")" '
    ($found | split("\n") | map(select(. != ""))) as $f
    | {rlib: $rlib, found: $f, expected: [$expected], ok: ($f == [$expected])}'
}

refresh_conclude() { # <step> <commit> <label>
  local p="$B_LOGS/refresh-$1.${HYPERFINE_ITERATION:?}"
  verify_base "$p.err" "$2" "$3" >"$p.verify.json"
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  "$@"
fi
