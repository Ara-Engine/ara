"""Finite ABBA save-to-visible probe with complete archived SDKs and actual reload checks."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import statistics
import subprocess
import time

from cpu_guard import CumulativeGuard, resources


def manifest(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(root.rglob("*")) if p.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["exe", "baseline-bin", "candidate-bin", "dotnet-root", "output"]:
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--edits", type=int, default=60)
    args = parser.parse_args()
    if args.edits < 3 or args.edits % 3:
        parser.error("edits must be a positive multiple of three")
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    sdk = {k: p.resolve() for k, p in [("baseline", args.baseline_bin), ("candidate", args.candidate_bin)]}
    frozen = {k: manifest(p) for k, p in sdk.items()}
    executable_hash = hashlib.sha256(args.exe.read_bytes()).hexdigest()
    scripts = {str(p.resolve()): hashlib.sha256(p.read_bytes()).hexdigest() for p in [Path(__file__), Path(__file__).with_name("cpu_guard.py")]}
    receipt = dict(accepted=False, executable=str(args.exe.resolve()), executable_sha256=executable_hash, scripts_sha256=scripts, dotnet_root=str(args.dotnet_root.resolve()), sdk_sha256=frozen, cells=[], limitations=["Sampled competing-build admission cannot establish absence of every short-lived foreign task.", "Historical archived runtime/compiler closures, not current-main performance.", "Save-to-visible includes the fixed 60 Hz tick boundary and ongoing simulation."])
    try:
        for index, label in enumerate(["baseline", "candidate", "candidate", "baseline"]):
            key = f"{index}-{label}"
            guard = CumulativeGuard()
            observations = []
            cell = dict(key=key, accepted=False, resources=observations)
            receipt["cells"].append(cell)
            first = resources()
            first["errors"] = guard.inspect(first)
            observations.append(first)
            if guard.rejected:
                raise RuntimeError("competing build before launch; no retry")
            result_path = out / (key + ".json")
            command = ["/usr/bin/nice", "-n", "10", str(args.exe.resolve()), "--edits", str(args.edits), "--lines", "5000", "--single-file", "--dotnet-bin", str(sdk[label]), "--output", str(result_path)]
            cell["command"] = command
            env = os.environ.copy()
            env.update(DOTNET_ROOT=str(args.dotnet_root.resolve()), DOTNET_HOST_TRACE="1", DOTNET_HOST_TRACEFILE=str(out / (key + ".hosttrace")), DOTNET_HOST_TRACE_VERBOSITY="4")
            with (out / (key + ".stdout")).open("w") as stdout, (out / (key + ".stderr")).open("w") as stderr:
                process = subprocess.Popen(command, env=env, stdout=stdout, stderr=stderr, start_new_session=True)
                deadline = time.monotonic() + 180
                try:
                    while True:
                        row = resources()
                        owned, foreign = [], []
                        for tool in row["busy_tools"]:
                            pgid = subprocess.run(["ps", "-p", str(tool["pid"]), "-o", "pgid="], text=True, capture_output=True, timeout=5).stdout.strip()
                            (owned if pgid == str(process.pid) else foreign).append(tool)
                        row["owned_tools"] = owned
                        row["busy_tools"] = foreign
                        row["errors"] = guard.inspect(row)
                        observations.append(row)
                        try:
                            cell["exit"] = process.wait(timeout=.1)
                            break
                        except subprocess.TimeoutExpired:
                            if time.monotonic() > deadline:
                                raise TimeoutError("reload process exceeded 180 seconds")
                finally:
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=10)
            final = resources()
            final["errors"] = guard.inspect(final)
            observations.append(final)
            data = json.loads(result_path.read_text())
            cell["quality"] = data["completed"] and data["success"] and data["measured_edits"] == args.edits and len(data["edits"]) == args.edits + 1 and not data["failures"] and data["leaked_generations"] == 0 and data["source"]["filler_files"] == 0 and data["source"]["probe_lines"] > 5000 and all(e["intended_kind"] == e["observed_kind"] and e["report"]["error_count"] == 0 for e in data["edits"])
            cell["stable"] = all(manifest(p) == frozen[k] for k, p in sdk.items()) and hashlib.sha256(args.exe.read_bytes()).hexdigest() == executable_hash and all(hashlib.sha256(Path(p).read_bytes()).hexdigest() == h for p, h in scripts.items())
            cell["stable_power"] = len({row["power"].splitlines()[0] for row in observations}) == 1 and "AC Power" in first["power"]
            cell["accepted"] = cell["exit"] == 0 and cell["quality"] and cell["stable"] and cell["stable_power"] and not guard.rejected
            cell["levels"] = {}
            for kind in [1, 2, 3]:
                values = sorted(e["save_to_visible_ms"] for e in data["edits"] if not e["warmup"] and e["observed_kind"] == kind)
                if len(values) != args.edits // 3:
                    raise RuntimeError("wrong per-level edit count")
                cell["levels"][kind] = dict(n=len(values), p50_ms=statistics.median(values), p95_ms=values[math.ceil(.95 * len(values)) - 1])
            print(json.dumps({k: v for k, v in cell.items() if k not in ["resources", "command"]}), flush=True)
            if not cell["accepted"]:
                raise RuntimeError("rejected live-edit cell; no retry")
        receipt["accepted"] = True
    except BaseException as error:
        receipt["failure"] = repr(error)
        raise
    finally:
        (out / "campaign.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
