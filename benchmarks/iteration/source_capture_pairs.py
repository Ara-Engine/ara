"""Four fresh actual-method allocation probes, ABBA; no clocks or idle-host admission."""
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
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    paths = {name: getattr(args, name).resolve() for name in ["baseline", "candidate", "helper", "dotnet_host"]}
    frozen = {name: manifest(path.parent) for name, path in paths.items() if name != "dotnet_host"}
    runtime_root = paths["dotnet_host"].parent / "shared/Microsoft.NETCore.App"
    runtime_files = manifest(runtime_root)
    host_hash = hashlib.sha256(paths["dotnet_host"].read_bytes()).hexdigest()
    source_paths = [Path(__file__).resolve(), *sorted(Path(__file__).with_name("source_hash").glob("*.cs"))]
    source_hashes = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in source_paths}
    report = dict(passed=False, timings_accepted=False, scope="Actual ProjectSourceCapture.Read, synchronous per-thread allocation counters. Reflection setup, fixture writes and validation excluded; pools/JIT warmed. Cold means no prior text, not a cold process.",
                  sdk_and_helper_sha256=frozen, host=str(paths["dotnet_host"]), host_sha256=host_hash,
                  runtime_sha256=runtime_files, source_sha256=source_hashes, processes=[])
    try:
        for slot, arm in enumerate(["baseline", "candidate", "candidate", "baseline"]):
            command = [str(paths["dotnet_host"]), str(paths["helper"]), str(paths[arm]),
                       "--project-sources", str(out / "FILES")]
            stdout = out / f"{slot}-{arm}.json"
            stderr = out / f"{slot}-{arm}.stderr"
            with stdout.open("w") as output, stderr.open("w") as errors:
                child = subprocess.Popen(command, stdout=output, stderr=errors)
                try:
                    exit_code = child.wait(timeout=120)
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=10)
            if exit_code or any(p.stat().st_size > 4 * 1024 * 1024 for p in [stdout, stderr]):
                raise RuntimeError("capture allocation helper failed: " + stderr.read_text()[:4096])
            data = json.loads(stdout.read_text())
            if len(data["rows"]) != 30 or any(len(row["allocatedBytesPerSet"]) != 20 for row in data["rows"]):
                raise RuntimeError("incorrect allocation sample inventory")
            report["processes"].append(dict(arm=arm, slot=slot, command=command, stdout_sha256=hashlib.sha256(stdout.read_bytes()).hexdigest(), **data))
        if len({row["runtime"] for row in report["processes"]}) != 1:
            raise RuntimeError("runtime version mismatch")
        report["passed"] = True
    except BaseException as error:
        report["failure"] = repr(error)
        raise
    finally:
        report["frozen_unchanged"] = all(manifest(paths[name].parent) == files for name, files in frozen.items()) and hashlib.sha256(paths["dotnet_host"].read_bytes()).hexdigest() == host_hash
        report["frozen_unchanged"] &= manifest(runtime_root) == runtime_files and all(hashlib.sha256(Path(p).read_bytes()).hexdigest() == h for p, h in source_hashes.items())
        report["passed"] &= report["frozen_unchanged"]
        (out / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        if not report["frozen_unchanged"]:
            raise RuntimeError("capture probe frozen inputs changed")


if __name__ == "__main__":
    main()
