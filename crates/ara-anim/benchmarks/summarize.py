"""Print Markdown timing tables from final JSONL measurements."""
import json
from pathlib import Path
import statistics

def rows(name):
    return [json.loads(line) for line in (Path(__file__).parent/name).read_text().splitlines()]

def cell(group,stage=False):
    p50=[float(row["stdout"].split(",")[3]) if stage else row["p50_ms"] for row in group]
    p95=[float(row["stdout"].split(",")[4]) if stage else row["p95_ms"] for row in group]
    load=[row["background_cpu_percent"] for row in group]
    return f"{statistics.median(p50):.3f}/{statistics.median(p95):.3f} ({min(load):.0f}-{max(load):.0f}%)"

if __name__=="__main__":
    stages=rows("stages.jsonl")
    for workers in [1,8,16]:
        print(f"\n{workers} workers: p50/p95 ms (other-process CPU)\n")
        print("| Stage | Ara before | Ara after | ozz |\n| --- | --- | --- | --- |")
        for stage in ["sample","blend","model","palette","all"]:
            cells=[cell([r for r in stages if r["workers"]==workers and r["stage"]==stage and r["engine"]==engine],True) for engine in ["baseline","candidate","ozz"]]
            print(f"| {stage} | "+" | ".join(cells)+" |")
    crowd=rows("crowd.jsonl")
    print("\n| Workers | Mode | Ara before | Ara after |\n| --- | --- | --- | --- |")
    for workers in [1,2,8,16]:
        for stage in ["animation","animation+palette"]:
            cells=[cell([r for r in crowd if r["workers"]==workers and r["stage"]==stage and r["engine"]==engine]) for engine in ["baseline","candidate"]]
            print(f"| {workers} | {stage} | "+" | ".join(cells)+" |")
