"""Prepare/run the existing S8 CPU kernels in an isolated Mac Unity Editor project."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import time

REPO = Path(__file__).resolve().parents[3]
SCRATCH = REPO / "target/perf-scale/unity-s8"
EDITOR = Path("/Applications/Unity/Hub/Editor/6000.4.5f1/Unity.app/Contents/MacOS/Unity")
MARKER = ".ara-s8-mac-scratch.json"
PACKAGES = {
    "com.unity.entities": "6.4.0",
    "com.unity.collections": "6.4.0",
    "com.unity.burst": "1.8.29",
    "com.unity.mathematics": "1.3.3",
}


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def scratch_path(value):
    path = value.resolve()
    if not path.is_relative_to((REPO / "target").resolve()):
        raise ValueError("Unity project and outputs must stay below this checkout's target/")
    return path


def prepare(args):
    root = scratch_path(args.project)
    marker = root / MARKER
    if root.exists() and any(root.iterdir()) and not marker.is_file():
        raise ValueError("refusing an existing project without this harness's scratch marker")
    if not args.editor.is_file():
        raise ValueError("installed Unity executable is missing")
    contents = args.editor.resolve().parents[1]
    with (contents / "Info.plist").open("rb") as stream:
        info = plistlib.load(stream)
    version = str(info["CFBundleShortVersionString"]).removeprefix("Unity version ")
    if version != "6000.4.5f1":
        raise ValueError("this scratch package manifest is pinned to installed Unity 6000.4.5f1")
    source = REPO / "spikes/s8-dots-bench/unity/Assets"
    # Copy our own fixture only. Existing user Unity projects and package caches are untouched.
    shutil.copytree(source, root / "Assets", dirs_exist_ok=True)
    shutil.copy2(Path(__file__).with_name("MacBatch.cs"), root / "Assets/Editor/MacBatch.cs")
    editor_assembly = root / "Assets/Editor/S8.Editor.asmdef"
    assembly = json.loads(editor_assembly.read_text())
    assembly["references"].append("Unity.Burst")
    write_json(editor_assembly, assembly)
    settings = root / "ProjectSettings"
    settings.mkdir(parents=True, exist_ok=True)
    (settings / "ProjectVersion.txt").write_text("m_EditorVersion: " + version + "\n", encoding="utf-8")
    write_json(root / "Packages/manifest.json", {"dependencies": PACKAGES})
    provenance = {
        "editor": str(args.editor.resolve()), "editor_version": version,
        "host": platform.platform(), "architecture": platform.machine(), "logical_cpus": os.cpu_count(),
        "requested_packages": PACKAGES, "source_project": "spikes/s8-dots-bench/unity",
        "kernel_sha256": {name: hashlib.sha256((source / name).read_bytes()).hexdigest()
                          for name in ["Benchmark.cs", "BenchSystem.cs", "Kernels.cs"]},
        "lane": "unity-editor-burst", "numeric_changes": False,
    }
    write_json(marker, provenance)
    print(json.dumps({"project": str(root), **provenance}, indent=2))


def active_editors():
    processes = subprocess.run(["ps", "-axo", "pid=,comm="], capture_output=True, text=True, check=True)
    return [line.strip() for line in processes.stdout.splitlines()
            if "/Unity.app/Contents/MacOS/Unity" in line]


def command(args):
    root = scratch_path(args.project)
    if not (root / MARKER).is_file():
        raise ValueError("run prepare first")
    if json.loads((root / MARKER).read_text())["editor"] != str(args.editor.resolve()):
        raise ValueError("Editor differs from prepared provenance; prepare with the requested Editor first")
    case = f"{args.workload}_{args.count}_t{args.threads}"
    destination = root / "results" / args.run
    result = destination / (case + ".json")
    log = destination / (case + ".log")
    warmup = args.warmup if args.warmup is not None else (3000 if args.workload == "w3" else 120)
    # This is a ceiling. The benchmark sets JobWorkerCount itself and records its actual value.
    workers = 14
    argv = ["/usr/bin/nice", "-n", "10", str(args.editor.resolve()), "-batchmode", "-nographics",
            "-projectPath", str(root), "-job-worker-count", str(workers),
            "-executeMethod", "AraIteration.MacBatch.Run", "-logFile", str(log), "-s8",
            args.workload, str(args.count), str(args.threads), str(warmup), str(args.frames),
            str(result), str(destination / (case + ".bin"))]
    environment = dict(os.environ)
    environment["UPM_CACHE_ROOT"] = str(root / "upm-cache")
    environment["UPM_GIT_LFS_CACHE_PATH"] = str(root / "upm-cache/git-lfs")
    # .NET's compiler IPC adds a random directory and socket name. macOS limits
    # Unix socket paths to 104 characters, so keep this scratch name short.
    environment["TMPDIR"] = str(root / "t")
    return root, destination, result, log, argv, environment


def run(args):
    root, destination, result, log, argv, environment = command(args)
    editors = active_editors()
    if editors:
        raise ValueError("a Unity Editor is already running; leave it untouched and retry later")
    if result.exists() or log.exists():
        raise ValueError("run name already has output for this case; choose a fresh --run")
    destination.mkdir(parents=True, exist_ok=True)
    Path(environment["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    with (destination / (result.stem + ".launcher.log")).open("w", encoding="utf-8") as output:
        with subprocess.Popen(argv, cwd=root, env=environment, stdout=output, stderr=subprocess.STDOUT) as child:
            try:
                status = child.wait(timeout=args.timeout)
            finally:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
    if status or not result.is_file():
        text = log.read_text(errors="replace").casefold() if log.exists() else ""
        license_failed = any(message in text for message in ["no valid unity editor license", "no valid license", "licensing initialization failed"])
        reason = "Unity license failure; no license changes attempted" if license_failed else "Unity failed before producing a result"
        raise RuntimeError(f"{reason} (exit {status}); inspect {log}")
    data = json.loads(result.read_text())
    metadata = json.loads(Path(str(result) + ".environment.json").read_text())
    if data["setup"] != "unity-editor" or not data["burst_enabled"]:
        raise RuntimeError("unexpected execution lane or Burst disabled")
    if (not metadata.get("measurement_completed") or not metadata.get("finally_restored")
            or metadata.get("burst_safety_checks") is not False
            or metadata.get("burst_force_safety_checks") is not False
            or metadata.get("burst_debug") is not False):
        raise RuntimeError("controlled Burst options/restoration missing; prepare the current wrapper and inspect metadata")
    write_json(destination / (result.stem + ".launch.json"), {
        "command": argv, "upm_cache_root": environment["UPM_CACHE_ROOT"],
        "wall_seconds_including_startup": time.monotonic() - started,
        "requested_threads": args.threads, "actual_job_workers": data["job_workers"],
        "job_worker_maximum": metadata["job_worker_maximum"], "nice": 10,
    })
    print(json.dumps({"result": str(result), "p50_ms": data["p50_ms"], "p95_ms": data["p95_ms"],
                      "requested_threads": args.threads, "actual_job_workers": data["job_workers"]}))


def verify(args):
    if args.result is None or args.reference is None:
        raise ValueError("verify requires --result <Unity.json> --reference <Ara.json>")
    result = scratch_path(args.result)
    reference = scratch_path(args.reference)
    root = scratch_path(args.project)
    if not result.is_relative_to(root):
        raise ValueError("Unity result must belong to this harness's scratch project")
    unity = json.loads(result.read_text())
    ara = json.loads(reference.read_text())
    for key in ["workload", "count", "warmup", "frames"]:
        if unity[key] != ara[key]:
            raise ValueError(f"different {key}; the final states are not comparable")
    dump = result.with_suffix(".bin")
    if dump.stat().st_size != unity["count"] * 12 * 4:
        raise ValueError("state dump size differs from S8's 12-float entity layout")
    libraries = [p for profile in ["perf", "release", "debug"]
                 for p in (REPO / "target" / profile / "deps").glob("libblake3-*.rlib")]
    if not libraries:
        raise ValueError("no existing BLAKE3 build artifact; compile the focused Ara ECS benchmark first")
    library = max(libraries, key=lambda path: path.stat().st_mtime_ns)
    output = root / "tools/dump-hash"
    output.parent.mkdir(parents=True, exist_ok=True)
    # Cached profiles differ: ordinary objects need no LTO, while thin-LTO
    # bitcode must be consumed by Rust's LLVM before Apple's older linker.
    errors = []
    for link_flags in [[], ["-C", "lto=thin"]]:
        compiled = subprocess.run(["/usr/bin/nice", "-n", "10", "rustc", "--edition=2024", *link_flags,
                                   str(Path(__file__).with_name("dump_hash.rs")), "--extern", "blake3=" + str(library),
                                   "-L", "dependency=" + str(library.parent), "-o", str(output)],
                                  capture_output=True, text=True)
        if compiled.returncode == 0:
            break
        errors.append(compiled.stderr)
    else:
        raise RuntimeError("cached BLAKE3 helper linking failed:\n" + "\n".join(errors))
    actual = subprocess.run([str(output), str(dump)], check=True, capture_output=True, text=True).stdout.strip()
    report = {"unity_result": str(result), "ara_result": str(reference), "algorithm": "BLAKE3",
              "hash_library": str(library), "hash_library_sha256": hashlib.sha256(library.read_bytes()).hexdigest(),
              "hash_helper_link_flags": link_flags,
              "ara_variant": ara["variant"], "ara_configured_pool_workers": ara["threads"],
              "ara_max_kernel_runners": 1 if ara["threads"] == 1 else ara["threads"] + 1,
              "unity_actual_job_workers": unity["job_workers"],
              "unity_max_kernel_runners": unity["job_workers"] + 1,
              "unity_state_hash": actual, "ara_state_hash": ara["state_hash"],
              "bit_identical": actual == ara["state_hash"]}
    write_json(result.with_suffix(".validation.json"), report)
    print(json.dumps(report))
    if not report["bit_identical"]:
        raise RuntimeError("final state differs; do not interpret timings as equivalent workloads")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "command", "run", "verify"])
    parser.add_argument("--project", type=Path, default=SCRATCH)
    parser.add_argument("--editor", type=Path, default=EDITOR)
    parser.add_argument("--workload", choices=["w1", "w3"], default="w1")
    parser.add_argument("--count", type=int, default=1_000_000)
    parser.add_argument("--threads", type=int, choices=[1, 15], default=1)
    parser.add_argument("--warmup", type=int)
    parser.add_argument("--frames", type=int, default=300)
    parser.add_argument("--run", default="mac-01")
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--result", type=Path)
    parser.add_argument("--reference", type=Path)
    args = parser.parse_args()
    if args.count < 64 or args.frames < 1 or args.timeout < 1 or (args.warmup is not None and args.warmup < 0):
        parser.error("count >= 64, frames/timeout > 0, warmup >= 0 required")
    if "/" in args.run or "\\" in args.run or args.run in {"", ".", ".."}:
        parser.error("--run must be one directory name")
    if args.action == "prepare":
        prepare(args)
    elif args.action == "command":
        _, _, _, _, argv, environment = command(args)
        print(json.dumps({"command": argv, "environment": {key: environment[key] for key in ["UPM_CACHE_ROOT", "UPM_GIT_LFS_CACHE_PATH"]}}, indent=2))
    elif args.action == "run":
        run(args)
    else:
        verify(args)


if __name__ == "__main__":
    main()
