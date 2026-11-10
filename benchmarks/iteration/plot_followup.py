"""Optional publication graphs; needs matplotlib, never part of an engine build."""
import argparse
import json
from pathlib import Path
import statistics
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.ticker import FuncFormatter

parser = argparse.ArgumentParser()
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
root = Path(__file__).resolve().parent
plt.rcParams.update({"font.family": "DejaVu Sans", "font.size": 11, "axes.spines.top": False, "axes.spines.right": False, "axes.spines.left": False, "axes.edgecolor": "#d4d9df", "text.color": "#172c42", "axes.labelcolor": "#172c42", "xtick.color": "#42566c", "ytick.color": "#42566c", "figure.facecolor": "white", "axes.facecolor": "white"})
colors = {"baseline": "#65758b", "candidate": "#0d9488"}
live = json.loads((root / "compiler_incremental_parse_mac_2026-11-10.json").read_text())
fig, ax = plt.subplots(figsize=(10.5, 5.8))
for arm, offset in [("baseline", -.2), ("candidate", .2)]:
    positions = [i + offset for i in range(3)]
    p50 = [live["summary"][str(i)][arm]["p50_ms"] for i in [1, 2, 3]]
    p95 = [live["summary"][str(i)][arm]["p95_ms"] for i in [1, 2, 3]]
    ax.bar(positions, p50, width=.35, color=colors[arm], label=arm.title(), zorder=3)
    ax.vlines(positions, p50, p95, color=colors[arm], linewidth=2, zorder=4)
    ax.scatter(positions, p95, marker="D", color=colors[arm], s=30, zorder=4)
    for x, median, tail in zip(positions, p50, p95):
        ax.text(x, median - 15, f"{median:.1f}", ha="center", color="white", weight="bold")
        ax.text(x, tail + 6, f"{tail:.1f}", ha="center", fontsize=10)
ax.set_xticks([0, 1, 2], ["L1 · body edit", "L2 · additive edit", "L3 · new module image"])
ax.set_ylim(0, 275)
ax.set_ylabel("Save to visible result (ms)")
ax.grid(axis="y", alpha=.17, zorder=0)
ax.legend(frameon=False, loc="upper left")
fig.suptitle("Live edits: completed forward and reverse pairs", x=.08, ha="left", fontsize=18, weight="bold")
ax.set_title("Bars: pooled p50   ◆: nearest-rank p95   ·   40 samples per arm / reload level", loc="left", fontsize=10, pad=16)
fig.text(.08, .045, "Mac · CoreCLR 10.0.14 · ABBA, four hosts, 240 correct edits, zero leaked generations\nArchived incremental-parse optimization; includes 60 Hz boundary. Not current-main or Unity latency.", fontsize=9, color="#53667a")
fig.subplots_adjust(left=.09, right=.98, bottom=.20, top=.80)
fig.savefig(args.output / "live-edits-abba.png", dpi=200)
plt.close(fig)

packet = json.loads((root / "source_hash_allocations_mac_2026-11-10.json").read_text())
fig, ax = plt.subplots(figsize=(11, 6.4))
names = ["384 B\nASCII", "768 B\nUnicode", "1,024 B\nboundary", "1,025 B\nboundary", "16 KiB", "512 KiB"]
for arm, offset in [("baseline", -.2), ("candidate", .2)]:
    values = [statistics.median(s["bytesPerCall"] for row in packet["processes"] if row["arm"] == arm for s in row["result"]["rows"][i]["samples"]) for i in range(6)]
    positions = [i + offset for i in range(6)]
    ax.bar(positions, values, width=.35, color=colors[arm], label=arm.title(), zorder=3)
    for x, value in zip(positions, values):
        label = f"{value:,.0f}" if value < 10000 else f"{value / 1024:,.1f} KiB"
        ax.text(x, value * 1.2, label, ha="center", fontsize=9)
ax.set_yscale("log")
ax.set_ylim(100, 2e6)
ax.set_xticks(range(6), names)
ax.yaxis.set_major_formatter(FuncFormatter(lambda y, _: f"{y:,.0f}"))
ax.set_ylabel("Allocated bytes per warmed call · logarithmic scale")
ax.grid(axis="y", alpha=.17, zorder=0)
ax.legend(frameon=False, loc="upper left")
fig.suptitle("Source fingerprints: bounded temporary buffers", x=.08, ha="left", fontsize=18, weight="bold")
ax.set_title("Direct production TextHash calls · per-thread allocation counters · median of raw batches", loc="left", fontsize=10, pad=16)
fig.text(.08, .045, "Mac · CoreCLR 10.0.14 · four processes ABBA, 20 batches/input/process, 32 warmups/input\n152 B includes the returned hexadecimal string. Exact UTF-8/SHA-256 hashes verified.\nAllocation evidence only: no admitted timing, complete-compile or retained-state p95 claim.", fontsize=9, color="#53667a")
fig.subplots_adjust(left=.15, right=.98, bottom=.23, top=.81)
fig.savefig(args.output / "source-hash-allocations.png", dpi=200)
plt.close(fig)

packet = json.loads((root / "sdk_hash_allocations_mac_2026-11-10.json").read_text())
fig, ax = plt.subplots(figsize=(10.5, 6.1))
for arm, offset in [("baseline", -.2), ("candidate", .2)]:
    values = [statistics.median(s["allocatedBytes"] for row in packet["processes"] if row["profile"] == profile and row["arm"] == arm for s in row["raw"]["samples"]) for profile in ["sdk", "65536", "1048576"]]
    positions = [i + offset for i in range(3)]
    ax.bar(positions, values, width=.35, color=colors[arm], label=arm.title(), zorder=3)
    for x, value in zip(positions, values):
        ax.text(x, value * 1.2, f"{value:,.0f}", ha="center", fontsize=10)
ax.set_yscale("log")
ax.set_ylim(1000, 2e7)
ax.set_xticks(range(3), ["Actual SDK\n4 assemblies", "4 × 64 KiB files", "4 × 1 MiB files"])
ax.yaxis.set_major_formatter(FuncFormatter(lambda y, _: f"{y:,.0f}"))
ax.set_ylabel("Allocated bytes per four-file set · logarithmic scale")
ax.grid(axis="y", alpha=.17, zorder=0)
ax.legend(frameon=False, loc="upper left")
fig.suptitle("SDK fingerprints: stream every byte", x=.08, ha="left", fontsize=18, weight="bold")
ax.set_title("Direct production HashFile vs former Hash(ReadAllBytes) · per-thread counters", loc="left", fontsize=10, pad=16)
fig.text(.08, .045, "Mac · CoreCLR 10.0.14 · 12 fresh processes, ABBA per set, 4 warmups + 20 measured sets/process\nAll digest vectors match; each file is fully read. Result arrays and strings are counted.\nAllocation evidence only: no admitted compiler, Play latency, peak-RSS or p95 claim.", fontsize=9, color="#53667a")
fig.subplots_adjust(left=.16, right=.98, bottom=.23, top=.81)
fig.savefig(args.output / "sdk-hash-allocations.png", dpi=200)
plt.close(fig)

packet = json.loads((root / "compiler_retained_gc_mac_2026-11-10.json").read_text())
fig, axes = plt.subplots(1, 2, figsize=(11, 5.9))
for ax, field, title in [(axes[0], "allocation_bytes_median", "Allocation per request"), (axes[1], "final_live_heap_bytes_median", "Live heap after final full GC")]:
    values = [packet["summary"][arm][field] / (1024 * 1024) for arm in ["baseline", "candidate"]]
    ax.bar([0, 1], values, color=[colors["baseline"], colors["candidate"]], width=.6, zorder=3)
    for i, value in enumerate(values):
        ax.text(i, value + max(values) * .025, f"{value:.1f} MiB", ha="center", fontsize=11)
    ax.set_xticks([0, 1], ["8-entry cache", "All 32 active modules"])
    ax.set_ylim(0, max(values) * 1.23)
    ax.set_title(title, loc="left", fontsize=12)
    ax.set_ylabel("MiB")
    ax.grid(axis="y", alpha=.17, zorder=0)
fig.suptitle("Retained compiler state: allocation and live heap", x=.08, ha="left", fontsize=17, weight="bold")
fig.text(.08, .045, "Mac · CoreCLR 10.0.7 · actual archived CLI + diagnostic hook · ABBA, 5 warmups + 10 requests/process\n1,024 scripts; final live heap follows collect/finalize/collect with compiler state and last reply rooted.\nNot peak RSS or uninstrumented steady state. No clocks admitted; retained-state p95 remains unresolved.", fontsize=9, color="#53667a")
fig.subplots_adjust(left=.08, right=.98, wspace=.28, bottom=.24, top=.80)
fig.savefig(args.output / "retained-cache-gc.png", dpi=200)
plt.close(fig)
