# Renders bench/results/<name>/results.json as Markdown (see bench/run.sh).
# Times are printed with millisecond precision (half-up), which is far below
# the run-to-run noise; nothing is rounded to a coarser unit.

def ms: if . == null then "–" else (. * 1000 | round) as $m
  | "\($m / 1000 | floor).\(("00" + ($m % 1000 | tostring))[-3:])" end;
def mib: if . == null then "–" else (. / 1048576 * 10 | round / 10 | tostring) + " MiB" end;
def counts: map(if . == null then "–" else tostring end) | join(", ");
def checks: if all(. == null) then "–"
  elif all(. == true) then "ok"
  else map(if . == true then "ok" elif . == false then "FAIL" else "–" end) | join(", ") end;

def sc_label: {
  "cold": "Cold worktree, empty target",
  "naive": "Naive copy (`cp -a` warm target into a fresh checkout)",
  "shared": "Shared `CARGO_TARGET_DIR`",
  "kache": "kache, warm local store",
  "ferry-off": "Ferry, sandbox off (crate-level reuse only)",
  "ferry-remap": "Ferry, remap-paths (sandbox off)",
  "ferry-overlay": "Ferry, overlay backend",
  "ferry-reflink": "Ferry, reflink backend",
  "ferry-copy": "Ferry, copy backend",
  "refresh": "Ferry base refresh",
  "refresh-off": "Ferry base refresh, sandbox off",
  "refresh-remap": "Ferry base refresh, remap-paths (sandbox off)"
}[.] // .;

def step_label: {
  "spawn_build": "first build after spawn, no edit",
  "edit_on_spawn": "**first edit after spawn** (spawn, edit, build)",
  "first_edit": "first edit after a spawn build (spawn, build, edit, build)",
  "second_edit": "second, different edit (steady state)",
  "base": "`ferry base` (cold)",
  "spawn": "`ferry spawn`",
  "concurrent": "agents with different edits building at once (wall)",
  "seeded_base": "`ferry base` after a commit, seeded from the previous base",
  "cold_base": "`ferry base --cold` after the same commit"
}[.] // .;

def order: ["cold", "naive", "shared", "kache", "ferry-off", "ferry-remap", "ferry-overlay", "ferry-reflink", "ferry-copy",
  "refresh", "refresh-off", "refresh-remap"];
def step_order: ["base", "spawn", "spawn_build", "edit_on_spawn", "first_edit", "second_edit", "concurrent", "seeded_base", "cold_base"];

. as $r
| $r.env as $e
| ($r.scenarios) as $s
| [order[] | select($s[.] != null)] as $present
| [
  "# Ferry benchmark: \($r.name)",
  "",
  (if $e.ci then
    "> **Shared CI runner** (\($e.ci.provider) `\($e.ci.image_os // "?") \($e.ci.image_version // "?")`, run \($e.ci.run_url // "unknown")). Times are noisy and **not comparable** with any other machine or any other run, including Ferry's published results. Compare scenarios only within this file. Crate counts, artifact checks and codegen-unit reuse do not depend on noise; read those first.",
    ""
  else empty end),
  "- Workspace: \($r.workspace.source) at `\($r.workspace.commit)`",
  "- Edit: append a documented `pub fn` to `\($r.workspace.edit_file)` (package `\($r.workspace.edit_package)`); every run uses a new, unique function",
  "- Build command: `\($r.workspace.build_command)` (debug), JSON messages; crates = `compiler-artifact` messages with `\"fresh\": false`",
  "- Runs per step: \($r.runs) (steps that are or prepare with a cold build: \($r.cold_runs)); concurrent agents: \($r.agents)",
  "",
  "## Environment",
  "",
  "| | |",
  "|---|---|",
  "| CPU | \($e.cpu_model) (\($e.physical_cores) cores, \($e.logical_cpus) logical CPUs, governor \($e.cpu_governor // "n/a")) |",
  "| RAM | \($e.ram_bytes / 1073741824 | round) GiB; available at start \($e.mem_available_bytes | mib)\(if $e.mem_available_note then " (" + $e.mem_available_note + ")" else "" end), swap in use at start \($e.swap_used_bytes | mib) |",
  "| OS / kernel | \($e.os) / \($e.kernel) |",
  "| Bench filesystem | \($e.bench_fs // "unknown") (`\($e.bench_mount.options // "n/a")`) |",
  (if $e.xcode then "| Xcode / linker | \($e.xcode) / `\($e.ld)` |" else empty end),
  "| rustc | `\($e.rustc | split("\n")[0])` |",
  "| cargo | `\($e.cargo)` |",
  "| ferry | `\($e.ferry)` at `\($e.ferry_commit)`\(if $e.ferry_source_dirty then " (uncommitted source changes)" else "" end) |",
  "| kache | `\($e.kache)` |",
  "| hyperfine | `\($e.hyperfine)` |",
  "| Date (UTC) | \($e.date) to \($e.date_end) |",
  "| Load average (1/5/15 min) | before \($e.loadavg_before), after \($e.loadavg_after) |",
  "",
  "## First edit after spawning a new agent",
  "",
  "A new agent is created, one edit is applied, and the timed build is the first build it runs.",
  "",
  "| Scenario | Median | Mean | Min | Max | Runs | Crates compiled | Artifact check |",
  "|---|---:|---:|---:|---:|---:|---|---|",
  ($present[] as $k | $s[$k].edit_on_spawn // empty | select(.runs != null)
    | "| \($k | sc_label) | \(.median | ms) s | \(.mean | ms) s | \(.min | ms) s | \(.max | ms) s | \(.runs) | \(.crates | counts) | \(.checks_ok | checks) |"),
  "",
  "## All timed steps",
  "",
  "| Scenario | Step | Median | Mean | Min | Max | Runs | Crates compiled (per run) | Check |",
  "|---|---|---:|---:|---:|---:|---:|---|---|",
  ($present[] as $k | step_order[] as $st | $s[$k][$st] // empty
    | if .skipped then "| \($k | sc_label) | \($st | step_label) | skipped: \(.skipped) | | | | | | |"
      else "| \($k | sc_label) | \($st | step_label) | \(.median | ms) s | \(.mean | ms) s | \(.min | ms) s | \(.max | ms) s | \(.runs) | \(.crates | counts) | \(.checks_ok | checks) |" end),
  "",
  "Check: the edited crate's rlib contains exactly the markers of this agent's own edits (none for a build without edits).",
  "",
  (if [$present[] | $s[.] | to_entries[] | select(.value.skipped or .value.failed)] | length > 0 then
    "## Skipped or failed", "",
    ($present[] as $k | $s[$k] | to_entries[] | select(.value.skipped or .value.failed)
      | "- \($k | sc_label), \(.key): \(if .value.skipped then "skipped: " + .value.skipped else "FAILED in step `\(.value.step)` (exit \(.value.exit_code)); logs are in the work dir" end)"),
    ""
  else empty end),
  "## Disk",
  "",
  "`du` counts every allocated block, including extents shared with the base; btrfs exclusive counts only blocks no other file shares.",
  (if $e.bench_fs == "apfs" then "On APFS, `du` counts cloned blocks in full; Ferry's private bytes are the better estimate of what an agent adds." else empty end),
  "",
  "| Scenario | What | du | btrfs exclusive | Ferry private bytes |",
  "|---|---|---:|---:|---:|",
  ($present[] as $k | $s[$k] as $x
    | ([["base_disk", "base target"], ["disk_after_spawn", "agent after spawn"], ["disk_after_second_edit", "agent after two edits"],
        ["target_disk", "target after first edit"], ["store_disk", "kache store"]][] as [$key, $what]
      | $x[$key] // empty
      | "| \($k | sc_label) | \($what) | \(.du_bytes | mib) | \(.btrfs_exclusive_bytes | mib) | \(.ferry_private_bytes | mib) |")),
  "",
  "## Concurrent builds",
  "",
  "| Scenario | Run | Wall | Per-agent elapsed | Crates per agent | Lock waits per agent (build dir / package cache) | Check |",
  "|---|---:|---:|---|---|---|---|",
  ($present[] as $k | $s[$k].concurrent // empty | select(.per_run) | . as $c
    | range(0; $c.runs) as $i | $c.per_run[$i].agents as $ag
    | "| \($k | sc_label) | \($i) | \($c.times[$i] | ms) s | \($ag | map(.elapsed | ms) | join(", ")) s | \($ag | map(.crates | tostring) | join(", ")) | \($ag | map("\(.build_dir_lock_waits)/\(.package_cache_lock_waits)") | join(", ")) | \([$c.checks_ok[$i]] | checks) |"),
  "",
  (if $s.shared.contamination then
    ($s.shared.contamination | . as $x
      | "## Shared-target contamination", "",
        "Agent B edits, agent A makes a different edit and builds, then B builds (single run).", "",
        "- A's build compiled \($x.a_crates) crate(s); B's build compiled \($x.b_crates): \($x.b_crate_list | join(", ") | if . == "" then "none" else . end)",
        "- B's rlib contains A's edit: **\($x.b_has_a_edit)**; contains B's own edit: **\($x.b_has_own_edit)**", "")
  else empty end),
  (if $s.kache then
    "## kache per-crate results", "",
    "From kache's `events.jsonl` for each timed build (cargo reports restored crates as compiled, so these are the real compiles).", "",
    "| Step | Run | Events by result | Compiled (not a cache hit) |",
    "|---|---:|---|---|",
    (step_order[] as $st | $s.kache[$st] // empty | select(.per_run) | . as $c
      | range(0; $c.runs) as $i | $c.per_run[$i].kache // empty
      | "| \($st) | \($i) | \(.by_result | to_entries | map("\(.key) \(.value)") | join(", ")) | \(.compiled | map(.crate + (if .reason then " (" + .reason + ")" else "" end)) | join(", ")) |"),
    ""
  else empty end),
  (if $s.kache.warm_build then "kache store warm-up build: \($s.kache.warm_build.seconds | ms) s, \($s.kache.warm_build.crates) crates (single run, not part of any timed step).", "" else empty end)
] | .[]
