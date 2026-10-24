#!/bin/sh
# Spawns N Ferry agents (or reuses existing ones) and runs the same command in
# each of them concurrently, then reports each agent's exit status and time.
#
# Usage (inside the repository, after `ferry base`):
#   parallel-agents.sh [-n N] [-p PREFIX] [--rm] [--] [COMMAND...]
#
#   -n N        number of agents (default 3)
#   -p PREFIX   agent names are PREFIX-1 ... PREFIX-N (default "agent")
#   --rm        remove the agents afterwards (`ferry rm`, which refuses
#               agents with uncommitted changes)
#   COMMAND     what to run in each agent (default: cargo test)
#
# Examples:
#   parallel-agents.sh
#   parallel-agents.sh -n 4 -- cargo clippy --all-targets
#   parallel-agents.sh -p review --rm -- cargo test --workspace
#
# Every run uses its own agent's warm target dir; none of them can change
# another's build state. Output goes to one log file per agent.
set -eu

n=3
prefix=agent
remove=0
while [ "$#" -gt 0 ]; do
    case $1 in
        -n) n=$2; shift 2 ;;
        -p) prefix=$2; shift 2 ;;
        --rm) remove=1; shift ;;
        -h | --help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        --) shift; break ;;
        *) break ;;
    esac
done
if [ "$#" -eq 0 ]; then
    set -- cargo test
fi
case $n in
    '' | *[!0-9]* | 0) echo "error: -n needs a positive number" >&2; exit 2 ;;
esac

logs=$(mktemp -d "${TMPDIR:-/tmp}/ferry-parallel.XXXXXX")
names=""
i=1
while [ "$i" -le "$n" ]; do
    name="$prefix-$i"
    # `ferry env` fails when the agent does not exist yet.
    if ! ferry env "$name" >/dev/null 2>&1; then
        ferry spawn "$name" >/dev/null
    fi
    names="$names $name"
    i=$((i + 1))
done

echo "running \`$*\` in$names" >&2
started=$(date +%s)
for name in $names; do
    (
        t0=$(date +%s)
        if ferry run "$name" -- "$@" >"$logs/$name.log" 2>&1; then
            status=0
        else
            status=$?
        fi
        echo "$status $(($(date +%s) - t0))" >"$logs/$name.status"
    ) &
done
wait
elapsed=$(($(date +%s) - started))

failed=0
printf '\n%-20s %-5s %-8s %s\n' AGENT EXIT SECONDS LOG
for name in $names; do
    read -r status secs <"$logs/$name.status"
    printf '%-20s %-5s %-8s %s\n' "$name" "$status" "$secs" "$logs/$name.log"
    if [ "$status" -ne 0 ]; then
        failed=$((failed + 1))
    fi
done
echo
echo "$((n - failed))/$n agents succeeded, ${elapsed}s wall time"

if [ "$remove" -eq 1 ]; then
    for name in $names; do
        ferry rm "$name" || echo "kept $name" >&2
    done
fi

[ "$failed" -eq 0 ]
