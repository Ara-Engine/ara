"""Read-only publication-discovery allocation/contract probe, four processes ABBA."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def manifest(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(root.rglob("*")) if p.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["baseline", "candidate", "helper", "dotnet-host", "output"]:
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--projects", type=Path, nargs="+", required=True)
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    paths = {name: getattr(args, name).resolve() for name in ["baseline", "candidate", "helper", "dotnet_host"]}
    projects = [p.resolve() for p in args.projects]
    trees = {name: path.parent for name, path in paths.items() if name != "dotnet_host"}
    trees["runtime"] = paths["dotnet_host"].parent / "shared/Microsoft.NETCore.App"
    trees |= {str(p): p for p in projects}
    frozen = {name: manifest(path) for name, path in trees.items()}
    host_hash = hashlib.sha256(paths["dotnet_host"].read_bytes()).hexdigest()
    report = dict(passed=False, timings_accepted=False, processes=[], frozen_sha256=frozen,
                  host=str(paths["dotnet_host"]), host_sha256=host_hash,
                  scope="Actual inventory method on existing Rust-published fixtures, read-only. Allocation and contract evidence only; no whole-compile latency or p95 claim.")
    try:
        for slot, arm in enumerate(["baseline", "candidate", "candidate", "baseline"]):
            command = [str(paths["dotnet_host"]), str(paths["helper"]), str(paths[arm]), *map(str, projects)]
            stdout, stderr = (out / f"{slot}-{arm}{suffix}" for suffix in [".json", ".stderr"])
            with stdout.open("w") as output, stderr.open("w") as errors:
                child = subprocess.Popen(command, stdout=output, stderr=errors)
                try:
                    exit_code = child.wait(timeout=120)
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=10)
            if exit_code or any(p.stat().st_size > 4 * 1024 * 1024 for p in [stdout, stderr]):
                raise RuntimeError("publication probe failed: " + stderr.read_text()[:4096])
            data = json.loads(stdout.read_text())
            expected_modes = {"public", "ownership", "gameplay-discovery", "editor-discovery"} if arm == "candidate" else {"public", "ownership"}
            if bool(data["scoped"]) != (arm == "candidate") or len(data["rows"]) != len(projects) * len(expected_modes):
                raise RuntimeError("incorrect discovery inventory")
            for project in projects:
                rows = [r for r in data["rows"] if r["project"] == str(project)]
                if {r["mode"] for r in rows} != expected_modes or any(len(r["allocations"]) != 20 for r in rows):
                    raise RuntimeError("incorrect discovery samples")
            report["processes"].append(dict(slot=slot, arm=arm, command=command, **data))
        for project in projects:
            for mode in ["public", "ownership"]:
                hashes = {r["responseSha256"] for p in report["processes"] for r in p["rows"]
                          if r["project"] == str(project) and r["mode"] == mode}
                if len(hashes) != 1:
                    raise RuntimeError("public/ownership response identity mismatch")
        if len({p["runtime"] for p in report["processes"]}) != 1:
            raise RuntimeError("runtime mismatch")
        report["passed"] = True
    except BaseException as error:
        report["failure"] = repr(error)
        raise
    finally:
        report["frozen_unchanged"] = all(manifest(path) == frozen[name] for name, path in trees.items()) and hashlib.sha256(paths["dotnet_host"].read_bytes()).hexdigest() == host_hash
        report["passed"] &= report["frozen_unchanged"]
        (out / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        if not report["frozen_unchanged"]:
            raise RuntimeError("probe changed frozen inputs or fixtures")


if __name__ == "__main__":
    main()
