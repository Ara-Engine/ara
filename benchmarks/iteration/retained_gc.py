"""Diagnostic-only actual-CLI GC counters. No latency admission or p95 result."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil

from compiler_scale import run_case


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["baseline", "candidate", "hook", "dotnet-host", "output"]:
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--modules", type=int, default=32)
    parser.add_argument("--files", type=int, default=32)
    parser.add_argument("--warmups", type=int, default=5)
    parser.add_argument("--samples", type=int, default=10)
    args = parser.parse_args()
    if min(args.modules, args.files, args.warmups, args.samples) < 1:
        parser.error("fixture counts must be positive")
    root = Path(__file__).resolve().parents[2]
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    sdk = {k: p.resolve() for k, p in [("baseline", args.baseline), ("candidate", args.candidate)]}
    paths = [p for server in sdk.values() for p in server.parent.rglob("*") if p.is_file()]
    paths += [args.hook.resolve(), Path(__file__).resolve(), Path(__file__).with_name("compiler_scale.py").resolve()]
    frozen = {str(p): sha(p) for p in paths}
    receipt = dict(timings_accepted=False, quality_passed=False, rows=[], frozen_sha256=frozen,
                   scope="Instrumented actual CLI; external load and hook overhead invalidate clocks. Final reply includes forced GC. Logger allocation is excluded, but logging can shift GC phase. Final live heap is collect/finalize/collect with compiler state and last reply rooted; not peak RSS or uninstrumented steady state.")
    try:
        for index, arm in enumerate(["baseline", "candidate", "candidate", "baseline"]):
            key = f"{index}-{arm}"
            log = out / (key + ".metrics.jsonl")
            os.environ.update(DOTNET_STARTUP_HOOKS=str(args.hook.resolve()), ARA_GC_LOG=str(log), ARA_GC_REQUESTS=str(args.warmups + args.samples))
            result = run_case(root, sdk[arm], args.modules, args.files, args.samples, str(args.dotnet_host.resolve()), key, False,
                              warmups=args.warmups, project_root=out / "PROJECT")
            metrics = [json.loads(line) for line in log.read_text().splitlines()]
            if len(metrics) != args.warmups + args.samples or metrics[-1]["finalLiveHeapBytes"] is None or any(row["request"] != i for i, row in enumerate(metrics)):
                raise RuntimeError("missing or repeated reply counters")
            artifacts = result["artifact_sha256_per_request"]
            if index == 0:
                reference = artifacts
            elif artifacts != reference:
                raise RuntimeError("exact PE/PDB/schema parity failed")
            if any(sha(Path(p)) != h for p, h in frozen.items()):
                raise RuntimeError("source or bundle changed")
            row = dict(slot=index, arm=arm, report=result, metrics=metrics)
            receipt["rows"].append(row)
            (out / (key + ".json")).write_text(json.dumps(row, indent=2) + "\n")
            print(json.dumps(dict(arm=arm, final_live_heap_bytes=metrics[-1]["finalLiveHeapBytes"])), flush=True)
            shutil.rmtree(out / "PROJECT")
        receipt["quality_passed"] = True
    except BaseException as error:
        receipt["failure"] = repr(error)
        raise
    finally:
        (out / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
