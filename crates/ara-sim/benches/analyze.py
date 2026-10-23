"""Validate result parity and summarize the two final S8 benchmark matrices."""
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).parent.parent / "bench-results"
rows = [json.loads(path.read_text(encoding="utf-8-sig")) for path in root.glob("run[12]/*.json") if not path.name.endswith(".load.json")]
if len(rows) != 40:
    raise SystemExit(f"Expected 40 final cases, found {len(rows)}")
groups = defaultdict(list)
for row in rows:
    groups[(row["workload"], row["count"], row["threads"])].append(row)

print("| Workload | Entities | Workers | Baseline p50 | Optimized p50 | Optimized p95 |")
print("|---|---:|---:|---:|---:|---:|")
for (workload, count, threads), group in sorted(groups.items()):
    before = [r for r in group if r["variant"] == "baseline"]
    after = [r for r in group if r["variant"] == "simd"]
    if len(before) != 2 or len(after) != 2:
        raise SystemExit(f"Incomplete group {workload}/{count}/{threads}")
    if len({r["state_hash"] for r in group}) != 1:
        raise SystemExit(f"State mismatch {workload}/{count}/{threads}")
    b = statistics.median(r["p50_ms"] for r in before)
    a = statistics.median(r["p50_ms"] for r in after)
    p95 = statistics.median(r["p95_ms"] for r in after)
    print(f"| {workload.upper()} | {count:,} | {threads} | {b:.3f} | {a:.3f} | {p95:.3f} |")

# Single/all-core must also finish with the same bits, not just each variant pair.
for workload, count in {(r["workload"], r["count"]) for r in rows}:
    if len({r["state_hash"] for r in rows if r["workload"] == workload and r["count"] == count}) != 1:
        raise SystemExit(f"Single/all-core state mismatch {workload}/{count}")

loads = [json.loads(p.read_text(encoding="utf-8-sig")) for p in root.glob("run[12]/*.load.json")]
cpu = [load["samples"][0]["cpuPercent"] for load in loads]
memory = [sample["freeKiB"] / 1024**2 for load in loads for sample in load["samples"]]
print(f"\n40/40 state hashes match across variants, repeats and worker counts.")
print(f"Pre-case CPU counters: {min(cpu)}–{max(cpu)}%, mean {statistics.mean(cpu):.1f}%.")
print(f"Free physical RAM: {min(memory):.2f}–{max(memory):.2f} GiB.")
print("Table values are medians of the two run percentiles, in milliseconds.")
