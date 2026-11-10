"""Finite Mac ABBA compilation experiment; artifact parity and sampled idle-host admission.

Uses complete frozen SDK closures, a single fixture path and identical revision numbers.
No source/bundle writes are timed. Rejections retain raw samples and are never retried.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

from cpu_guard import CumulativeGuard, resources


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def closure(path):
    return {str(p.relative_to(path)): sha(p) for p in sorted(path.rglob("*")) if p.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--dotnet-host", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--modules", type=int, required=True)
    parser.add_argument("--files", type=int, required=True)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--warmups", type=int, default=5)
    parser.add_argument("--dependency-fanout", action="store_true")
    parser.add_argument("--quality-only", action="store_true", help="two small arms; ignore load admission, accept no timings")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    fixture = out / "PROJECT"
    servers = {k: p.resolve() for k, p in [("baseline", args.baseline), ("candidate", args.candidate)]}
    frozen = {k: closure(p.parent) for k, p in servers.items()}
    scripts = {str(p): sha(p) for p in [Path(__file__).resolve(), Path(__file__).with_name("compiler_scale.py").resolve(), Path(__file__).with_name("cpu_guard.py").resolve()]}
    report = dict(accepted=False, case="dependency" if args.dependency_fanout else "retained", config=vars(args).copy(), sdk_sha256=frozen, scripts_sha256=scripts, cells=[], limitations=["Sampled competing-build admission, not scheduler tracing or proof of zero background activity.", "Historical binary closures; this isolates the reported optimization stage, not current-main performance.", "Nearest-rank p95 from twenty requests per process remains an exploratory tail estimate."])
    report["config"] = {k: str(v) if isinstance(v, Path) else v for k, v in report["config"].items()}
    try:
        for index, label in enumerate(["baseline", "candidate"] if args.quality_only else ["baseline", "candidate", "candidate", "baseline"]):
            key = f"{index}-{label}"
            guard = CumulativeGuard()
            snapshots = []
            cell = dict(key=key, accepted=False, resources=snapshots)
            report["cells"].append(cell)
            first = resources()
            first["errors"] = guard.inspect(first)
            snapshots.append(first)
            if guard.rejected and not args.quality_only:
                raise RuntimeError("competing build before launch; no retry")
            command = [sys.executable, str(Path(__file__).with_name("compiler_scale.py")), "--server", str(servers[label]), "--dotnet-host", str(args.dotnet_host.resolve()), "--modules", str(args.modules), "--files", str(args.files), "--samples", str(args.samples), "--warmups", str(args.warmups), "--project-root", str(fixture), "--output", str(out / (key + ".json"))]
            if args.dependency_fanout:
                command.append("--dependency-fanout")
            cell["command"] = command
            with (out / (key + ".stdout")).open("w") as stdout, (out / (key + ".stderr")).open("w") as stderr:
                process = subprocess.Popen(command, stdout=stdout, stderr=stderr, start_new_session=True)
                deadline = time.monotonic() + 600
                try:
                    while True:
                        row = resources()
                        foreign, owned = [], []
                        for tool in row["busy_tools"]:
                            pgid = subprocess.run(["ps", "-p", str(tool["pid"]), "-o", "pgid="], text=True, capture_output=True, timeout=5).stdout.strip()
                            (owned if pgid == str(process.pid) else foreign).append(tool)
                        row["owned_tools"] = owned
                        row["busy_tools"] = foreign
                        row["errors"] = guard.inspect(row)
                        snapshots.append(row)
                        try:
                            cell["exit"] = process.wait(timeout=.1)
                            break
                        except subprocess.TimeoutExpired:
                            if time.monotonic() > deadline:
                                raise TimeoutError("cell exceeded 600 seconds")
                finally:
                    # Only this launch's process group is owned, including its SDK child.
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=10)
            last = resources()
            last["errors"] = guard.inspect(last)
            snapshots.append(last)
            cell["stable_bundles"] = all(closure(p.parent) == frozen[k] for k, p in servers.items())
            cell["stable_scripts"] = all(sha(Path(p)) == h for p, h in scripts.items())
            cell["stable_power"] = len({r["power"].splitlines()[0] for r in snapshots}) == 1 and "AC Power" in first["power"]
            cell["quality_passed"] = cell["exit"] == 0 and (args.quality_only or not guard.rejected) and cell["stable_bundles"] and cell["stable_scripts"] and cell["stable_power"]
            cell["accepted"] = cell["quality_passed"] and not args.quality_only
            if not cell["quality_passed"]:
                raise RuntimeError(f"rejected {key}; raw durations unaccepted, no retry")
            data = json.loads((out / (key + ".json")).read_text())
            cell["summary"] = {k: data[k] for k in ["p50_ms", "p95_ms", "first_ms", "peak_rss_bytes", "runtime"]}
            artifacts = data["artifact_sha256_per_request"]
            if index == 0:
                reference = artifacts
            elif artifacts != reference:
                raise RuntimeError("exact PE/PDB/schema parity failed; timings unaccepted")
            cell["exact_artifact_parity"] = True
            print(json.dumps(dict(key=key, **cell["summary"])), flush=True)
            shutil.rmtree(fixture)  # Owned generated fixture only; next arm uses the identical absolute path.
        report["quality_passed"] = True
        report["accepted"] = not args.quality_only
    except BaseException as error:
        report["failure"] = repr(error)
        raise
    finally:
        (out / "campaign.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
