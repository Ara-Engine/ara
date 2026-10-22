"""Validate every float in short-run dumps and summarize raw frame measurements."""
import argparse
import json
from pathlib import Path
import numpy as np

ROOT = Path(__file__).resolve().parent
parser = argparse.ArgumentParser()
parser.add_argument("--validate", action="store_true")
parser.add_argument("--long", action="store_true", help="Compare full final state after a measurement run")
parser.add_argument("--run", default="run1")
parser.add_argument("--reference-run", help="Use another run's Rust dumps, for optional editor comparisons")
parser.add_argument("--complete", action="store_true", help="Require the whole 44-case matrix")
args = parser.parse_args()

def require_complete(names):
    names = set(names)
    expected = set()
    for setup in ("rust", "jit", "aot", "unity"):
        cases = [("w1", 1000000), ("w2", 20000), ("w2", 50000), ("w3", 1000000), ("w4", 100000)]
        if setup in ("jit", "aot"):
            cases.append(("w3simd", 1000000))
        for workload, count in cases:
            for threads in (1, 16):
                expected.add(f"{setup}_{workload}_{count}_t{threads}")
    if args.complete and names != expected:
        raise AssertionError(f"Missing {sorted(expected - names)}; unexpected {sorted(names - expected)}")
    if args.complete:
        print(f"Complete matrix: {len(expected)} cases")

if args.validate:
    reports = []
    folder = ROOT / "results" / args.run if args.long else ROOT / "results/validation" / args.run
    for path in sorted(folder.glob("*.json")):
        if path.name.endswith(".load.json"):
            continue
        row = json.loads(path.read_text(encoding="utf-8-sig"))
        workload = row["workload"].replace("simd", "")
        ref_name = f"rust_{workload}_{row['count']}_t1"
        ref = np.fromfile(ROOT / "out" / f"{args.reference_run or args.run}_{ref_name}.bin", dtype="<f4")
        actual = np.fromfile(ROOT / "out" / f"{args.run}_{path.stem}.bin", dtype="<f4")
        if ref.shape != actual.shape or ref.size != row["count"] * 12:
            raise AssertionError(f"shape mismatch: {path.name}")
        # Fixed absolute + relative tolerance, with no aggregate cancellation.
        error = np.abs(ref - actual)
        failures = ~np.isfinite(actual) | (error > 2e-4 + 2e-5 * np.abs(ref))
        report = {"case": path.stem, "floats": int(ref.size), "max_abs_error": float(error.max()), "failures": int(failures.sum()), "pass": not bool(failures.any())}
        reports.append(report)
        print(report)
    prefix = "validation-long" if args.long else "validation"
    (ROOT / "results" / f"{prefix}-{args.run}.json").write_text(json.dumps(reports, indent=2) + "\n")
    if not reports or not all(r["pass"] for r in reports):
        raise SystemExit("Cross-runtime equivalence FAILED")
    require_complete(r["case"] for r in reports)
else:
    names = []
    for path in sorted((ROOT / "results" / args.run).glob("*.json")):
        if path.name.endswith(".load.json"):
            continue
        r = json.loads(path.read_text(encoding="utf-8-sig"))
        names.append(path.stem)
        ms = np.array(r["ms"])
        assert len(ms) == r["frames"] == 300 and np.all(np.isfinite(ms)) and np.all(ms > 0)
        first, last = np.median(ms[:100]), np.median(ms[-100:])
        print(f"{path.stem:32} {r['p50_ms']:8.3f} / {r['p95_ms']:8.3f} ms; first/last={first/last:5.2f}; p95/p50={r['p95_ms']/r['p50_ms']:4.2f}")
    require_complete(names)
