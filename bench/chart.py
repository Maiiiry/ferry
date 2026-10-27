#!/usr/bin/env python3
"""Draws bench/results/chart.svg from one or more bench results.json files.

One group of horizontal bars per workspace: the median time of the first
build after spawning a new agent and making one edit, per scenario. Each
group has its own x axis (workspaces differ by orders of magnitude), so
compare bars within a group, not across groups.

Standard library only. Usage:
    python3 bench/chart.py bench/results/ripgrep/results.json bench/results/rust-analyzer/results.json
"""

import argparse
import json
import math
import sys
from html import escape
from pathlib import Path

# (scenario keys in results.json, first one measured wins; row label, emphasized)
ROWS = [
    (("cold",), "Cold worktree", False),
    (("naive",), "Copied warm target", False),
    (("kache",), "kache", False),
    # Reflink is what `auto` picks on the benchmark machine's btrfs.
    (("ferry-reflink", "ferry-overlay"), "Ferry", True),
]
DEFAULT_OUT = Path(__file__).parent / "results" / "chart.svg"

# Transparent background, so every color must read on GitHub's light
# (#ffffff) and dark (#0d1117) pages. Checked with the dataviz palette
# validator: both marks clear 3:1 on both surfaces. The gray is a neutral
# context color by design (emphasis: highlight Ferry, gray the rest).
TEXT = "#7b7b7b"  # about 4.3:1 on both surfaces
GRID = "#7b7b7b"
FERRY = "#3987e5"
OTHER = "#8c8c87"

FONT = "system-ui, -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif"
WIDTH = 760
LABEL_W = 236  # row label column
VALUE_W = 150  # room for the value printed after the longest bar
ROW_H = 28
BAR_H = 18
RADIUS = 4


def nice_step(max_value, max_ticks=5):
    raw = max_value / max_ticks
    mag = 10 ** math.floor(math.log10(raw))
    for mult in (1, 2, 2.5, 5, 10):
        if raw <= mult * mag:
            return mult * mag
    return 10 * mag


def fmt_secs(value):
    # Neutral half-up rounding for every bar; exact values are in results.md.
    if value < 10:
        return f"{value:.2f} s"
    if value < 100:
        return f"{value:.1f} s"
    return f"{value:.0f} s"


def fmt_tick(value):
    return f"{value:g}"


def crates_text(crates):
    known = [c for c in crates if c is not None]
    if not known:
        return ""
    lo, hi = min(known), max(known)
    n = f"{lo}" if lo == hi else f"{lo}-{hi}"
    return f" · {n} crate{'s' if hi != 1 else ''}"


def bar_path(x0, y, width, height, r):
    """A bar from the baseline x0 with a rounded data end and a square base."""
    if width <= 0:
        return ""
    r = min(r, width, height / 2)
    x1 = x0 + width
    return (
        f"M{x0:.1f},{y:.1f}H{x1 - r:.1f}Q{x1:.1f},{y:.1f} {x1:.1f},{y + r:.1f}"
        f"V{y + height - r:.1f}Q{x1:.1f},{y + height:.1f} {x1 - r:.1f},{y + height:.1f}"
        f"H{x0:.1f}Z"
    )


def load(paths, metric):
    groups = []
    for path in paths:
        data = json.loads(Path(path).read_text())
        rows = []
        for keys, label, emphasized in ROWS:
            scenarios = data.get("scenarios", {})
            key = next((k for k in keys if scenarios.get(k, {}).get(metric)), keys[0])
            step = scenarios.get(key, {}).get(metric)
            if key == "kache" and data["env"].get("kache"):
                label = data["env"]["kache"]  # e.g. "kache 1.2.2"
            if step and step.get("median") is not None:
                rows.append(
                    {
                        "label": label,
                        "emphasized": emphasized,
                        "median": step["median"],
                        "min": step["min"],
                        "max": step["max"],
                        "runs": step["runs"],
                        "crates": step.get("crates", []),
                    }
                )
            else:
                rows.append({"label": label, "emphasized": emphasized, "median": None})
        groups.append({"data": data, "rows": rows})
    return groups


def env_caption(env):
    fs = env.get("bench_fs", "?")
    rustc = env.get("rustc", "").split("\n")[0]
    return (
        f"{'shared CI runner (noisy) · ' if env.get('ci') else ''}"
        f"{env.get('cpu_model', '?')}, {env.get('physical_cores', '?')} cores / "
        f"{env.get('logical_cpus', '?')} threads, {fs}, {rustc}"
    )


def render(groups, metric):
    out = []
    y = 0

    def text(x, y, s, size=13, anchor="start", weight="normal", extra=""):
        out.append(
            f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" text-anchor="{anchor}" '
            f'font-weight="{weight}" fill="{TEXT}"{extra}>{escape(s)}</text>'
        )

    title = {
        "edit_on_spawn": "First edit after spawning a new agent",
        "first_edit": "First edit after a new agent's first build",
    }.get(metric, metric)
    y += 24
    text(0, y, title, size=17, weight="600")
    y += 20
    text(0, y, "Median wall time of the first build that includes the agent's edit. Lower is better.", size=12)
    y += 18

    plot_x0 = LABEL_W
    plot_w = WIDTH - LABEL_W - VALUE_W
    for group in groups:
        data = group["data"]
        known = [r["median"] for r in group["rows"] if r["median"] is not None]
        if not known:
            continue
        step = nice_step(max(known))
        domain = step * math.ceil(max(known) / step)
        scale = plot_w / domain

        y += 22
        ws = data["workspace"]
        runs = sorted({r["runs"] for r in group["rows"] if r["median"] is not None})
        runs_text = ", ".join(str(n) for n in runs)
        text(0, y, data["name"], size=14, weight="600")
        text(
            plot_x0,
            y,
            f"edit {ws['edit_file']} · {ws['build_command']} · median of {runs_text} run{'s' if runs != [1] else ''}",
            size=11,
        )
        y += 10
        top = y
        bottom = top + ROW_H * len(group["rows"])
        ticks = [i * step for i in range(int(round(domain / step)) + 1)]
        for t in ticks:
            x = plot_x0 + t * scale
            out.append(
                f'<line x1="{x:.1f}" y1="{top:.1f}" x2="{x:.1f}" y2="{bottom:.1f}" '
                f'stroke="{GRID}" stroke-opacity="0.35" stroke-width="1"/>'
            )
            text(x, bottom + 14, fmt_tick(t), size=11, anchor="middle")
        text(plot_x0 + plot_w, bottom + 28, "seconds", size=11, anchor="end")

        for i, row in enumerate(group["rows"]):
            row_y = top + i * ROW_H
            bar_y = row_y + (ROW_H - BAR_H) / 2
            mid = bar_y + BAR_H / 2 + 4.5
            text(plot_x0 - 10, mid, row["label"], anchor="end", weight="600" if row["emphasized"] else "normal")
            if row["median"] is None:
                text(plot_x0 + 6, mid, "not measured", size=12, extra=' font-style="italic"')
                continue
            width = row["median"] * scale
            tip = (
                f"{row['label']}: median {row['median']:.3f} s "
                f"(min {row['min']:.3f}, max {row['max']:.3f}, n={row['runs']}){crates_text(row['crates'])}"
            )
            color = FERRY if row["emphasized"] else OTHER
            out.append(f"<g><title>{escape(tip)}</title>")
            if width >= 1:
                out.append(f'<path d="{bar_path(plot_x0, bar_y, width, BAR_H, RADIUS)}" fill="{color}"/>')
            else:  # a sub-pixel value still gets a visible tick at the baseline
                out.append(f'<rect x="{plot_x0:.1f}" y="{bar_y:.1f}" width="1" height="{BAR_H}" fill="{color}"/>')
            out.append("</g>")
            text(plot_x0 + max(width, 1) + 6, mid, fmt_secs(row["median"]) + crates_text(row["crates"]), size=12)
        # Baseline.
        out.append(
            f'<line x1="{plot_x0:.1f}" y1="{top:.1f}" x2="{plot_x0:.1f}" y2="{bottom:.1f}" '
            f'stroke="{GRID}" stroke-width="1"/>'
        )
        y = bottom + 34

    captions = []
    for group in groups:
        c = env_caption(group["data"]["env"])
        if c not in captions:
            captions.append(c)
    y += 6
    for c in captions:
        y += 16
        text(0, y, c, size=11)
    y += 16
    text(
        0,
        y,
        "Each group has its own axis. Bars: median; hover for min, max and run count. Desktop machine, not a lab.",
        size=11,
    )
    y += 12

    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{y:.0f}" '
        f'viewBox="0 0 {WIDTH} {y:.0f}" font-family="{FONT}" role="img" '
        f'aria-label="{escape(title)}">\n<title>{escape(title)}</title>\n' + "\n".join(out) + "\n</svg>\n"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("results", nargs="+", help="results.json files, one per workspace")
    parser.add_argument("--metric", default="edit_on_spawn", help="step to chart (default: edit_on_spawn)")
    parser.add_argument("--out", default=str(DEFAULT_OUT))
    args = parser.parse_args()
    groups = load(args.results, args.metric)
    if not any(r["median"] is not None for g in groups for r in g["rows"]):
        sys.exit(f"no {args.metric} measurements in the given results")
    # Shared-runner times are not comparable with a dedicated machine's, and
    # the default chart is the one the README shows.
    ci = {bool(g["data"]["env"].get("ci")) for g in groups}
    if len(ci) > 1:
        sys.exit("the results mix shared CI runners with other machines; chart them separately")
    if True in ci and Path(args.out).resolve() == DEFAULT_OUT.resolve():
        sys.exit(f"results from a shared CI runner are never written to {DEFAULT_OUT}; pass --out")
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(render(groups, args.metric))
    print(args.out)


if __name__ == "__main__":
    main()

# Automated fix for issue #37: Base mode decisions use the main worktree's toolchain, not the checkout's
