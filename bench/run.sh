#!/usr/bin/env bash
# Ferry benchmark harness: new-agent build times with and without Ferry.
# Methodology and scenario definitions: bench/README.md.
set -euo pipefail
# $B_BUILD_ARGS is split into words on purpose: it holds extra cargo arguments.
# shellcheck disable=SC2086

usage() {
  cat <<'EOF'
Usage: bench/run.sh --workspace <git url or path> --edit <relative file> [options]

  --workspace W    git URL, git checkout, or a plain directory (copied and committed)
  --rev R          commit to benchmark (default: HEAD of W; a plain directory's snapshot)
  --edit F         file to edit, relative to the workspace root; must belong to a lib crate
  --runs N         timed runs per step (default 5)
  --cold-runs N    runs for steps that are, or prepare with, a cold build (default: --runs)
  --agents N       concurrent agents (default 3)
  --out DIR        results directory (default bench/results/<name>)
  --name NAME      name for the work dir and results (default: workspace basename)
  --scenarios L    comma-separated subset of: cold naive shared kache ferry-off
                   ferry-overlay ferry-reflink ferry-copy refresh (default: all)
  --build-args A   extra arguments for every `cargo build` (default: none)
  --ferry PATH     ferry binary (default: build this checkout with --release)
EOF
}

B_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
B_LIB=$B_ROOT/bench/lib.sh
# shellcheck source=lib.sh
source "$B_LIB"

ALL_SCENARIOS="cold naive shared kache ferry-off ferry-overlay ferry-reflink ferry-copy refresh"
WORKSPACE='' REV='' B_EDIT='' RUNS=5 COLD_RUNS='' B_AGENTS=3 B_OUT='' NAME='' SCENARIOS=all
B_BUILD_ARGS='' B_FERRY=''
KACHE_VERSION=1.2.2

while (($#)); do
  case $1 in
    --workspace) WORKSPACE=$2 ;;
    --rev) REV=$2 ;;
    --edit) B_EDIT=$2 ;;
    --runs) RUNS=$2 ;;
    --cold-runs) COLD_RUNS=$2 ;;
    --agents) B_AGENTS=$2 ;;
    --out) B_OUT=$2 ;;
    --name) NAME=$2 ;;
    --scenarios) SCENARIOS=$2 ;;
    --build-args) B_BUILD_ARGS=$2 ;;
    --ferry) B_FERRY=$2 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      die "unknown argument $1"
      ;;
  esac
  shift 2
done
[[ -n $WORKSPACE && -n $B_EDIT ]] || {
  usage >&2
  exit 2
}
[[ $RUNS =~ ^[1-9][0-9]*$ && $B_AGENTS =~ ^[1-9][0-9]*$ ]] || die "--runs and --agents must be positive integers"
COLD_RUNS=${COLD_RUNS:-$RUNS}
[[ $COLD_RUNS =~ ^[1-9][0-9]*$ ]] || die "--cold-runs must be a positive integer"

# Canonical order: ferry-off runs before the namespace backends because it
# builds its own base, and refresh needs the namespace base and moves HEAD.
if [[ $SCENARIOS == all ]]; then
  SELECTED=$ALL_SCENARIOS
else
  SELECTED=''
  for s in $ALL_SCENARIOS; do
    [[ ",$SCENARIOS," == *",$s,"* ]] && SELECTED+="$s "
  done
  for s in ${SCENARIOS//,/ }; do
    [[ " $ALL_SCENARIOS " == *" $s "* ]] || die "unknown scenario $s"
  done
fi

for tool in git cargo rustc hyperfine jq du findmnt; do
  command -v "$tool" >/dev/null || die "$tool not found on PATH"
done

NAME=${NAME:-$(basename "${WORKSPACE%/}" .git)}
B_WORK=$B_ROOT/bench/work/$NAME
B_REPO=$B_WORK/repo
B_LOGS=$B_WORK/logs
B_STATE=$B_WORK/state
B_OUT=$(mkdir -p "${B_OUT:-$B_ROOT/bench/results/$NAME}" && cd "${B_OUT:-$B_ROOT/bench/results/$NAME}" && pwd)
B_KACHE=$B_ROOT/bench/.cache/kache-$KACHE_VERSION/bin/kache
rm -rf "$B_LOGS" "$B_STATE" "$B_OUT/hyperfine"
mkdir -p "$B_WORK/wt" "$B_LOGS" "$B_STATE/steps" "$B_OUT/hyperfine"

# Nothing from the caller's environment may redirect or wrap the builds.
unset CARGO_TARGET_DIR CARGO_BUILD_BUILD_DIR CARGO_BUILD_TARGET_DIR RUSTC_WRAPPER \
  RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_RUSTC_WRAPPER RUSTFLAGS CARGO_ENCODED_RUSTFLAGS \
  CARGO_INCREMENTAL CARGO_PROFILE_DEV_INCREMENTAL FERRY_HOME FERRY_WORKTREES FERRY_BACKEND \
  FERRY_SANDBOX FERRY_CONFIG FERRY_FRESHNESS FERRY_BASE_COMMAND FERRY_REMAP_PATHS FERRY_AGENT
export B_ROOT B_LIB B_WORK B_REPO B_REV B_EDIT B_LIB_SRC B_BUILD_ARGS B_FERRY B_LOGS B_OUT \
  B_STATE B_AGENTS B_KACHE

# --- tools ---------------------------------------------------------------------

if [[ -z $B_FERRY ]]; then
  note "building ferry (release)"
  cargo build --quiet --release --manifest-path "$B_ROOT/Cargo.toml"
  B_FERRY=$B_ROOT/target/release/ferry
fi
B_FERRY=$(realpath "$B_FERRY")

if [[ " $SELECTED " == *" kache "* && ! -x $B_KACHE ]]; then
  note "installing kache $KACHE_VERSION into bench/.cache"
  CARGO_TARGET_DIR=$B_ROOT/bench/.cache/kache-build cargo install --quiet kache \
    --version "$KACHE_VERSION" --locked --root "$B_ROOT/bench/.cache/kache-$KACHE_VERSION"
fi

# --- workspace -------------------------------------------------------------------

SOURCE_DESC=$WORKSPACE
if [[ -d $WORKSPACE && ! -e $WORKSPACE/.git ]]; then
  # A plain directory such as tests/fixtures/workspace: benchmark a committed copy.
  if [[ ! -d $B_REPO/.git ]]; then
    mkdir -p "$B_REPO"
    cp -a "$WORKSPACE"/. "$B_REPO"/
    rm -rf "$B_REPO/target"
    git -C "$B_REPO" init --quiet --initial-branch=bench
    git -C "$B_REPO" add -A
    git -C "$B_REPO" -c user.name=bench -c user.email=bench@localhost commit --quiet -m "bench: workspace snapshot"
  fi
  REV=${REV:-$(git -C "$B_REPO" rev-list --max-parents=0 HEAD | tail -n 1)}
  SOURCE_DESC="copy of $(realpath "$WORKSPACE")"
else
  if [[ -d $WORKSPACE ]]; then
    MIRROR=$(realpath "$WORKSPACE")
  else
    MIRROR=$B_ROOT/bench/.cache/repos/$(basename "${WORKSPACE%/}" .git).git
    [[ -d $MIRROR ]] || git clone --quiet --bare "$WORKSPACE" "$MIRROR"
    if [[ -n $REV ]] && ! git -C "$MIRROR" cat-file -e "$REV^{commit}" 2>/dev/null; then
      git -C "$MIRROR" fetch --quiet origin '+refs/heads/*:refs/heads/*'
    fi
  fi
  [[ -d $B_REPO/.git ]] || git clone --quiet --no-checkout "$MIRROR" "$B_REPO"
  if [[ -n $REV ]] && ! git -C "$B_REPO" cat-file -e "$REV^{commit}" 2>/dev/null; then
    git -C "$B_REPO" fetch --quiet origin
  fi
  REV=${REV:-$(git -C "$MIRROR" rev-parse HEAD)}
fi
git -C "$B_REPO" config user.name bench
git -C "$B_REPO" config user.email bench@localhost
git -C "$B_REPO" worktree prune
git -C "$B_REPO" checkout --quiet --force -B bench "$REV"
B_REV=$(git -C "$B_REPO" rev-parse HEAD)

note "fetching dependencies"
(cd "$B_REPO" && cargo fetch --quiet)

# The edited file's crate root, used to find its rlib in cargo's output.
d=$(dirname "$B_EDIT")
while :; do
  if [[ -f $B_REPO/$d/Cargo.toml ]] && grep -q '^\[package\]' "$B_REPO/$d/Cargo.toml"; then break; fi
  [[ $d == . ]] && die "no package contains $B_EDIT"
  d=$(dirname "$d")
done
REPO_REAL=$(realpath "$B_REPO")
B_LIB_SRC=$(cd "$B_REPO" && cargo metadata --no-deps --format-version 1 |
  jq -r --arg m "$(realpath "$B_REPO/$d/Cargo.toml")" '.packages[] | select(.manifest_path == $m)
    | .targets[] | select(.kind | any(. == "lib" or . == "rlib")) | .src_path')
[[ -n $B_LIB_SRC ]] || die "$B_EDIT is not part of a library crate"
B_LIB_SRC=${B_LIB_SRC#"$REPO_REAL/"}
EDIT_PACKAGE=$(cd "$B_REPO" && cargo metadata --no-deps --format-version 1 |
  jq -r --arg m "$(realpath "$B_REPO/$d/Cargo.toml")" '.packages[] | select(.manifest_path == $m) | .name')

# --- environment -------------------------------------------------------------------

capture_env() {
  local ferry_commit ferry_dirty=false mount
  ferry_commit=$(git -C "$B_ROOT" rev-parse HEAD)
  [[ -n $(git -C "$B_ROOT" status --porcelain -- src Cargo.toml Cargo.lock) ]] && ferry_dirty=true
  mount=$(findmnt -J -T "$B_WORK" -o TARGET,SOURCE,FSTYPE,OPTIONS | jq -c '.filesystems[0]')
  jq -n \
    --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg cpu "$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ *//')" \
    --argjson cpus "$(nproc)" \
    --argjson cores "$(lscpu -p=core,socket 2>/dev/null | grep -v '^#' | sort -u | wc -l)" \
    --arg governor "$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || true)" \
    --argjson mem "$(awk '/^MemTotal/ {print $2 * 1024}' /proc/meminfo)" \
    --argjson mem_avail "$(awk '/^MemAvailable/ {print $2 * 1024}' /proc/meminfo)" \
    --argjson swap_used "$(awk '/^SwapTotal/ {t=$2} /^SwapFree/ {f=$2} END {print (t - f) * 1024}' /proc/meminfo)" \
    --arg kernel "$(uname -r)" \
    --arg os "$(. /etc/os-release && echo "$PRETTY_NAME")" \
    --arg fstype "$(stat -f -c %T "$B_WORK")" \
    --argjson mount "$mount" \
    --argjson free "$(df --output=avail -B1 "$B_WORK" | tail -n 1)" \
    --arg rustc "$(cd "$B_REPO" && rustc -vV)" \
    --arg cargo "$(cd "$B_REPO" && cargo -V)" \
    --arg ferry "$("$B_FERRY" --version)" --arg ferry_path "$B_FERRY" \
    --arg ferry_commit "$ferry_commit" --argjson ferry_dirty "$ferry_dirty" \
    --arg kache "$([[ -x $B_KACHE ]] && "$B_KACHE" --version 2>/dev/null || echo 'not installed')" \
    --arg hyperfine "$(hyperfine --version)" \
    --arg git "$(git --version)" \
    --arg cp "$(cp --version | head -n 1)" \
    --arg btrfs "$(btrfs --version 2>/dev/null | head -n 1 || true)" \
    --arg load "$(loadavg)" \
    '{date: $date, cpu_model: $cpu, logical_cpus: $cpus, physical_cores: $cores, cpu_governor: $governor,
      ram_bytes: $mem, mem_available_bytes: $mem_avail, swap_used_bytes: $swap_used,
      kernel: $kernel, os: $os, bench_fs: $fstype, bench_mount: $mount, bench_free_bytes: $free,
      rustc: $rustc, cargo: $cargo, ferry: $ferry, ferry_path: $ferry_path, ferry_commit: $ferry_commit,
      ferry_source_dirty: $ferry_dirty, kache: $kache, hyperfine: $hyperfine, git: $git, cp: $cp,
      btrfs_progs: $btrfs, loadavg: $load}'
}

capture_env >"$B_STATE/env-before.json"
note "load average before: $(loadavg)"

# --- scenario helpers ------------------------------------------------------------------

lib() { echo "bash $(printf %q "$B_LIB") $*"; }

# One hyperfine invocation; per-run logs are collected into a step record.
hf() { # <scenario> <step> <runs> <prepare> <command> [conclude]
  local name="$1@$2" before aux="$B_LOGS/$1-$2.aux.log"
  # hyperfine discards the output of --prepare and --conclude; keep it.
  local args=(--runs "$3" --shell=bash --style basic --export-json "$B_OUT/hyperfine/$1-$2.json"
    --command-name "$1/$2" --prepare "$4 >>$(printf %q "$aux") 2>&1")
  [[ -n ${6:-} ]] && args+=(--conclude "$6 >>$(printf %q "$aux") 2>&1")
  before=$(loadavg)
  note "$1/$2: $3 run(s)"
  echo "$2" >"$B_STATE/current-step"
  if ! hyperfine "${args[@]}" "$5" >"$B_LOGS/$1-$2.hyperfine.log" 2>&1; then
    tail -n 30 "$B_LOGS/$1-$2.hyperfine.log" "$aux" >&2
    die "$1/$2 failed; see $B_LOGS/$1-$2.hyperfine.log, $aux and the build output in $B_LOGS/$1-$2.*.err"
  fi
  collect "$1" "$2" "$before" "$(loadavg)"
  jq -r --arg n "$1/$2" '"bench:   \($n): median \(.median * 1000 | round / 1000) s, min \(.min * 1000 | round / 1000) s, max \(.max * 1000 | round / 1000) s, n=\(.runs), crates \(.crates | map(tostring) | join(",")), checks \(.checks_ok | map(tostring) | join(","))"' \
    "$B_STATE/steps/$name.json" >&2
}

collect() { # <scenario> <step> <load before> <load after>
  local hfj="$B_OUT/hyperfine/$1-$2.json" n i p log report runs="$B_STATE/$1@$2.runs.jsonl"
  n=$(jq '.results[0].times | length' "$hfj")
  : >"$runs"
  for ((i = 0; i < n; i++)); do
    p="$B_LOGS/$1-$2.$i"
    case $2 in
      base | seeded_base | cold_base) log=$p.err report=$p.json ;;
      spawn) log='' report=$p.json ;;
      concurrent) log='' report='' ;;
      *) log=$p.json report='' ;;
    esac
    jq -n -c \
      --argjson crates "$(if [[ -n $log ]]; then crate_list "$log"; else echo null; fi)" \
      --argjson report "$(if [[ -n $report ]]; then jq -c . "$report"; else echo null; fi)" \
      --argjson verify "$(cat "$p.verify.json" 2>/dev/null || echo null)" \
      --argjson kache "$(cat "$p.kache.json" 2>/dev/null || echo null)" \
      --argjson agents "$(cat "$p.agents.json" 2>/dev/null || echo null)" \
      '{crates: (if $crates == null then null else ($crates | length) end), crate_list: $crates,
        verify: $verify, kache: $kache, agents: $agents, ferry_report: $report}' >>"$runs"
  done
  jq -s -c --slurpfile hf "$hfj" --arg lb "$3" --arg la "$4" --arg hfpath "hyperfine/$1-$2.json" '
    ($hf[0].results[0]) as $r
    | {runs: ($r.times | length), times: $r.times, median: $r.median, mean: $r.mean, min: $r.min,
       max: $r.max, stddev: $r.stddev, exit_codes: $r.exit_codes,
       crates: map(.crates),
       checks_ok: map(if .verify != null then .verify.ok
                      elif .agents != null and (.agents | any(.verify != null)) then (.agents | all(.verify.ok))
                      else null end),
       per_run: ., hyperfine: $hfpath, load_before: $lb, load_after: $la}' "$runs" >"$B_STATE/steps/$1@$2.json"
}

put() { # <scenario> <key> <json>: an extra record for the scenario
  echo "$3" >"$B_STATE/steps/$1@$2.json"
}

build_steps() { # <scenario> <step>...
  local sc=$1 step runs
  shift
  for step in "$@"; do
    runs=$RUNS
    [[ $sc == cold ]] && runs=$COLD_RUNS
    hf "$sc" "$step" "$runs" "$(lib prep "$sc" "$step")" "$(timed_build_cmd "$sc" "$step")" "$(lib conclude "$sc" "$step")"
  done
}

free_bytes() { df --output=avail -B1 "$B_WORK" | tail -n 1; }

# Records a skipped step instead of silently leaving it out.
skip() { # <scenario> <step> <reason>
  note "$1/$2: skipped: $3"
  put "$1" "$2" "$(jq -n -c --arg r "$3" '{skipped: $r}')"
}

# --- scenarios --------------------------------------------------------------------------

sc_cold() {
  build_steps cold spawn_build edit_on_spawn first_edit
  put cold target_disk "$(disk_json "$B_WORK/wt/cold/target")"
  wt_remove "$B_WORK/wt/cold"
}

sc_naive() {
  wt_new "$B_WORK/wt/naive-donor"
  (cd "$B_WORK/wt/naive-donor" && exec cargo build $B_BUILD_ARGS --message-format=json) \
    >"$B_LOGS/naive-donor.json" 2>"$B_LOGS/naive-donor.err" || die "naive donor build failed"
  build_steps naive spawn_build edit_on_spawn first_edit
  wt_remove "$B_WORK/wt/naive"
  wt_remove "$B_WORK/wt/naive-donor"
}

sc_shared() {
  ((B_AGENTS >= 2)) || die "shared needs --agents >= 2"
  local a
  shared_setup
  hf shared concurrent "$RUNS" "$(lib shared_conc_prep)" "$(lib shared_conc_run)" "$(lib shared_conc_conclude)"
  put shared contamination "$(shared_contamination)"
  for a in $(seq 1 "$B_AGENTS"); do wt_remove "$B_WORK/wt/shared-$a"; done
  rm -rf "$B_WORK/shared-target" "$B_WORK/shared-snap"
}

sc_kache() {
  local k=$B_WORK/kache
  rm -rf "$k"
  mkdir -p "$k/cache" "$k/run" "$k/xdg"
  : >"$k/empty.toml"
  export RUSTC_WRAPPER=$B_KACHE KACHE_CACHE_DIR=$k/cache KACHE_RUNTIME_DIR=$k/run \
    KACHE_CONFIG=$k/empty.toml KACHE_HOST_CONFIG='' XDG_CACHE_HOME=$k/xdg
  # The daemon's socket path must be at most 107 bytes.
  export KACHE_SOCKET_PATH=${XDG_RUNTIME_DIR:-/tmp}/ferry-bench-kache-$$.sock
  "$B_KACHE" daemon start >"$B_LOGS/kache-daemon.log" 2>&1
  trap '"$B_KACHE" daemon stop >/dev/null 2>&1 || true' EXIT
  wt_new "$B_WORK/wt/kache-donor"
  local t0 t1
  t0=$(date +%s.%N)
  (cd "$B_WORK/wt/kache-donor" && exec cargo build $B_BUILD_ARGS --message-format=json) \
    >"$B_LOGS/kache-warm.json" 2>"$B_LOGS/kache-warm.err" || die "kache warm build failed"
  t1=$(date +%s.%N)
  put kache warm_build "$(jq -n -c --arg s "$t0" --arg e "$t1" --argjson c "$(crate_list "$B_LOGS/kache-warm.json")" \
    '{note: "single untimed-by-hyperfine build that fills the kache store", seconds: (($e | tonumber) - ($s | tonumber)), crates: ($c | length)}')"
  build_steps kache spawn_build edit_on_spawn first_edit second_edit
  put kache store_disk "$(disk_json "$k/cache")"
  "$B_KACHE" daemon stop >/dev/null 2>&1 || true
  trap - EXIT
  wt_remove "$B_WORK/wt/kache"
  wt_remove "$B_WORK/wt/kache-donor"
  rm -rf "$k"
}

ferry_env() { # <variant>: overlay, reflink, copy or off
  export FERRY_HOME=$B_WORK/ferry/home FERRY_WORKTREES=$B_WORK/ferry/wt
  # The base warms exactly what every scenario builds; JSON so its crates can be counted.
  export FERRY_BASE_COMMAND="cargo build $B_BUILD_ARGS --message-format=json"
  if [[ $1 == off ]]; then
    export FERRY_SANDBOX=off FERRY_BACKEND=auto
  else
    export FERRY_SANDBOX=namespace FERRY_BACKEND=$1
  fi
}

ferry_base_cmd() { # <scenario> <step> [extra base args]
  local out="$B_LOGS/$1-$2"
  echo "exec $(printf %q "$B_FERRY") -C $(printf %q "$B_REPO") base --json ${3:-} >$(printf %q "$out").\$HYPERFINE_ITERATION.json 2>$(printf %q "$out").\$HYPERFINE_ITERATION.err"
}

sc_ferry() { # <variant>
  local v=$1 sc=ferry-$1 base_dir need state
  ferry_env "$v"
  if [[ $v == overlay || $v == off ]]; then
    hf "$sc" base "$COLD_RUNS" "$(lib ferry_drop_bases)" "$(ferry_base_cmd "$sc" base)"
  else
    fq base --json >"$B_LOGS/$sc-ensure-base.json" 2>"$B_LOGS/$sc-ensure-base.err"
  fi
  base_dir=$(ls -d "$(ferry_store)/bases/$B_REV"-*/ | head -n 1)
  put "$sc" base_disk "$(disk_json "${base_dir%/}/target")"
  if [[ $v == copy ]]; then
    need=$(du -s -B1 "${base_dir%/}/target" | cut -f1)
    if (($(free_bytes) < need + (2 << 30))); then
      skip "$sc" all "free space $(free_bytes) B < one copied target ($need B) + 2 GiB"
      return
    fi
  fi
  hf "$sc" spawn "$RUNS" "$(lib ferry_rm bench)" \
    "exec $(printf %q "$B_FERRY") -C $(printf %q "$B_REPO") spawn bench --json >$(printf %q "$B_LOGS/$sc-spawn").\$HYPERFINE_ITERATION.json 2>$(printf %q "$B_LOGS/$sc-spawn").\$HYPERFINE_ITERATION.err"
  state=$(ferry_agent_state_dir bench)
  put "$sc" disk_after_spawn "$(disk_json "$state")"
  build_steps "$sc" spawn_build edit_on_spawn first_edit second_edit
  put "$sc" disk_after_second_edit "$(jq -n -c --argjson d "$(disk_json "$state")" \
    --argjson p "$(fq list --json | jq '.agents[] | select(.name == "bench") | .private_bytes')" '$d + {ferry_private_bytes: $p}')"
  ferry_rm bench
  if [[ $v == copy ]]; then
    # Per agent: what a fresh copy really occupied (btrfs exclusive bytes; the
    # kernel may share extents), else its full size.
    need=$(jq '.btrfs_exclusive_bytes // .du_bytes' "$B_STATE/steps/$sc@disk_after_spawn.json")
    need=$((need * B_AGENTS))
  else
    need=0
  fi
  if (($(free_bytes) < need + (2 << 30))); then
    skip "$sc" concurrent "free space $(free_bytes) B < $B_AGENTS copied targets ($need B) + 2 GiB"
  else
    hf "$sc" concurrent "$RUNS" "$(lib ferry_conc_prep "$sc")" "$(lib ferry_conc_run "$sc")" "$(lib ferry_conc_conclude "$sc")"
  fi
  ferry_rm bench $(seq -f 'c%g' 1 "$B_AGENTS")
  if [[ $v == off ]]; then ferry_drop_bases; fi
}

sc_refresh() {
  local c0=$B_REV c1 l
  ferry_env overlay
  fq base --json >"$B_LOGS/refresh-ensure-base.json" 2>"$B_LOGS/refresh-ensure-base.err"
  l=$(label refresh commit 0)
  edit "$B_REPO" "$l"
  git -C "$B_REPO" commit --quiet -am "bench: refresh edit"
  c1=$(git -C "$B_REPO" rev-parse HEAD)
  put refresh commits "$(jq -n -c --arg a "$c0" --arg b "$c1" '{previous: $a, refreshed: $b}')"
  hf refresh seeded_base "$RUNS" "$(lib ferry_drop_bases "$c1")" "$(ferry_base_cmd refresh seeded_base)" \
    "$(lib refresh_conclude seeded_base "$c1" "$l")"
  hf refresh cold_base "$COLD_RUNS" "$(lib ferry_drop_bases "$c1")" "$(ferry_base_cmd refresh cold_base --cold)" \
    "$(lib refresh_conclude cold_base "$c1" "$l")"
  ferry_drop_bases
  git -C "$B_REPO" reset --quiet --hard "$c0"
}

run_scenario() { # <scenario>
  local rc
  note "=== $1 (load $(loadavg), free $(($(free_bytes) >> 20)) MiB)"
  echo setup >"$B_STATE/current-step"
  # Not `( ... ) || rc=$?`: bash ignores `set -e` inside a subshell whose
  # status is tested, so a failing command would not stop the scenario.
  set +e
  (
    set -e
    case $1 in
      ferry-*) sc_ferry "${1#ferry-}" ;;
      *) "sc_$1" ;;
    esac
  )
  rc=$?
  set -e
  if ((rc != 0)); then
    note "$1 FAILED (exit $rc); continuing with the next scenario"
    put "$1" error "$(jq -n -c --argjson rc "$rc" --arg step "$(cat "$B_STATE/current-step" 2>/dev/null || true)" \
      --arg logs "$B_LOGS" '{failed: true, exit_code: $rc, step: $step, logs: $logs}')"
  fi
}

# Leftovers of an interrupted earlier run: agents, locked base worktrees, targets.
reset_work() {
  local w
  while read -r w; do
    [[ $w == "$REPO_REAL" ]] || git -C "$B_REPO" worktree unlock "$w" 2>/dev/null || true
  done < <(git -C "$B_REPO" worktree list --porcelain | sed -n 's/^worktree //p')
  chmod -R u+rwx "$B_WORK/ferry" "$B_WORK/wt" 2>/dev/null || true
  rm -rf "$B_WORK/ferry" "$B_WORK/wt" "$B_WORK/kache" "$B_WORK/shared-target" "$B_WORK/shared-snap"
  mkdir -p "$B_WORK/wt"
  git -C "$B_REPO" worktree prune
}

# --- results ------------------------------------------------------------------------------

assemble() {
  capture_env >"$B_STATE/env-after.json"
  jq -s '.[0] + {loadavg_before: .[0].loadavg, loadavg_after: .[1].loadavg,
          mem_available_after_bytes: .[1].mem_available_bytes, swap_used_after_bytes: .[1].swap_used_bytes,
          date_end: .[1].date,
          idle_note: "idle means no other heavy jobs; judge from loadavg_before/after (1, 5, 15 min) against logical_cpus"}
        | del(.loadavg)' \
    "$B_STATE/env-before.json" "$B_STATE/env-after.json" >"$B_OUT/env.json"
  local f
  for f in "$B_STATE"/steps/*.json; do
    [[ -e $f ]] || continue
    f=$(basename "$f" .json)
    jq -c --arg sc "${f%@*}" --arg key "${f#*@}" '{sc: $sc, key: $key, value: .}' "$B_STATE/steps/$f.json"
  done | jq -s --slurpfile env "$B_OUT/env.json" \
    --arg name "$NAME" --arg source "$SOURCE_DESC" --arg rev "$B_REV" --arg edit "$B_EDIT" \
    --arg lib_src "$B_LIB_SRC" --arg pkg "$EDIT_PACKAGE" --arg build "cargo build $B_BUILD_ARGS" \
    --argjson runs "$RUNS" --argjson cold_runs "$COLD_RUNS" --argjson agents "$B_AGENTS" \
    --arg selected "$SELECTED" '
    {schema: 1, name: $name,
     workspace: {source: $source, commit: $rev, edit_file: $edit, edit_package: $pkg, edit_crate_root: $lib_src,
                 build_command: ($build | rtrimstr(" "))},
     runs: $runs, cold_runs: $cold_runs, agents: $agents,
     scenarios_selected: ($selected | split(" ") | map(select(. != ""))),
     env: $env[0],
     scenarios: (group_by(.sc) | map({key: .[0].sc, value: (map({key: .key, value: .value}) | from_entries)}) | from_entries)}' \
    >"$B_OUT/results.json"
  jq -r -f "$B_ROOT/bench/results-md.jq" "$B_OUT/results.json" >"$B_OUT/results.md"
}

reset_work
for s in $SELECTED; do
  run_scenario "$s"
  assemble
done
note "load average after: $(loadavg)"
note "results: $B_OUT/results.md"
