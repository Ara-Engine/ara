#!/usr/bin/env python3
"""Fresh-process ABBA comparison of SDK variants through `reload_bench`.

Each run starts a new process: `startup_ms` (host start + initial compile) and the
per-edit rows (save -> visible) come from the bench's `--output` JSON. Variants run in
mirrored order (A B C C B A, repeated) so drift hits every variant equally. Raw rows are kept.

  python3 benchmarks/iteration/startup_pairs.py --exe target/release/examples/reload_bench \
      --variant base=/abs/sdk-base --variant r2r=/abs/sdk-r2r --rounds 3 --edits 30 --output /abs/out.json
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
from pathlib import Path


def pct(values, p):
    values = sorted(values)
    return values[round((len(values) - 1) * p)] if values else float("nan")


def run(exe, sdk, edits, lines, out, env):
    cmd = [exe, "--dotnet-bin", sdk, "--edits", str(edits), "--lines", str(lines), "--output", str(out)]
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=600, env={**os.environ, **env})
    return json.loads(Path(out).read_text())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--variant", action="append", required=True, help="name=/absolute/projects-bin")
    ap.add_argument("--env", action="append", default=[], help="variant:KEY=VALUE, set only for that variant")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--edits", type=int, default=30)
    ap.add_argument("--lines", type=int, default=5000)
    ap.add_argument("--output", required=True)
    a = ap.parse_args()
    variants = [v.split("=", 1) for v in a.variant]
    order = [n for n, _ in variants] + [n for n, _ in reversed(variants)]
    sdk = dict(variants)
    env = {n: {} for n in sdk}
    for item in a.env:
        name, kv = item.split(":", 1)
        k, v = kv.split("=", 1)
        env[name][k] = v
    scratch = Path(a.output).with_suffix(".runs")
    scratch.mkdir(parents=True, exist_ok=True)
    runs = []
    for r in range(a.rounds):
        for i, name in enumerate(order):
            d = run(a.exe, sdk[name], a.edits, a.lines, scratch / f"{r}-{i}-{name}.json", env[name])
            if not d.get("success"):
                print("FAILED", name, d.get("failures"), file=sys.stderr)
            edits = [e for e in d["edits"] if not e["warmup"] and e["save_to_visible_ms"] is not None]
            runs.append({"variant": name, "round": r, "startup_ms": d["startup_ms"], "success": d.get("success"),
                         "edits": [{"index": e["edit"], "kind": e["observed_kind"], "ms": e["save_to_visible_ms"],
                                    "first_tick_ms": e["first_tick_ms"], "report": e["report"]} for e in edits]})
            print(name, round(d["startup_ms"]), flush=True)
    summary = {}
    for name, _ in variants:
        mine = [r for r in runs if r["variant"] == name]
        s = [r["startup_ms"] for r in mine]
        entry = {"startup_ms": {"n": len(s), "median": statistics.median(s), "p95": pct(s, 0.95), "min": min(s), "max": max(s)}}
        for kind, label in ((1, "L1"), (2, "L2"), (3, "L3")):
            v = [e["ms"] for r in mine for e in r["edits"] if e["kind"] == kind]
            entry[label] = {"n": len(v), "median": statistics.median(v) if v else None, "p95": pct(v, 0.95) if v else None}
            for ph in ("emit_us", "analyze_us", "generate_us", "server_us"):
                pv = [e["report"][ph] / 1000 for r in mine for e in r["edits"] if e["kind"] == kind and e["report"]]
                entry[label][ph.replace("_us", "_ms_median")] = statistics.median(pv) if pv else None
        summary[name] = entry
    Path(a.output).write_text(json.dumps({"order": order, "runs": runs, "summary": summary}, indent=1))
    print(json.dumps(summary, indent=1))


if __name__ == "__main__":
    main()
