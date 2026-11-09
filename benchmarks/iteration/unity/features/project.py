"""Isolated project preparation and exact commands; importing this starts no tools."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys

sys.dont_write_bytecode = True
import inputs
import cases
import recovery
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
PROJECT = ROOT / "target/perf-scale/uf"
RUST_TARGET = ROOT / "target/perf-scale/feature-rust"
EXTENDED_PROJECT = ROOT / "target/perf-scale/ufx"
EXTENDED_RUST_TARGET = ROOT / "target/perf-scale/extended-feature-rust"
MARKER = ".ara-query-feature.json"
spec = importlib.util.spec_from_file_location("ara_s8_helpers", HERE.parent / "bench.py")
BASE = importlib.util.module_from_spec(spec)
spec.loader.exec_module(BASE)


def template_inputs():
    return inputs.templates(HERE, HERE.parent / "MacBatch.cs", HERE.parent / "bench.py")


def prepared_manifest(args):
    root = BASE.scratch_path(args.project)
    manifest = inputs.check(root, MARKER, template_inputs(), str(BASE.EDITOR), BASE.PACKAGES)
    expected = "extended-v1" if cases.extended(args) else "query_filter-v1"
    if manifest["input_manifest"]["fixture"] != expected:
        raise ValueError("prepared project belongs to another fixture family; choose its separate project or run prepare")
    return manifest


def paths(args, engine):
    lane = execution_lane(args)
    root = BASE.scratch_path(args.project)
    stem = f"{cases.name(args)}_{args.count}_{cases.variant(args)}_r{args.runners}_{engine}"
    if lane == "correctness-smoke":
        stem += "_smoke"
    folder = root / "results" / args.run
    return folder / (stem + ".json"), folder / (stem + ".bin"), folder / (stem + ".log")


def execution_lane(args):
    if not getattr(args, "smoke", False):
        return "timed"
    if not 64 <= args.count <= 10_000 or args.warmup < 0 or args.frames < 1 or args.warmup + args.frames > 10:
        raise ValueError("correctness smoke requires 64..10000 entities and at most 10 total steps")
    return "correctness-smoke"


def prepare(args):
    root = BASE.scratch_path(args.project)
    marker = root / MARKER
    if root.exists() and any(root.iterdir()) and not marker.is_file():
        raise ValueError("refusing an unmarked existing project")
    if not BASE.EDITOR.is_file():
        raise ValueError("the pinned Unity executable is not installed")
    with (BASE.EDITOR.parents[1] / "Info.plist").open("rb") as source:
        version = str(plistlib.load(source)["CFBundleShortVersionString"]).removeprefix("Unity version ")
    if version != "6000.4.5f1":
        raise ValueError("feature packages require the pinned installed Editor")
    # Reuse the reviewed control wrapper without changing the frozen S8 source.
    template = (HERE.parent / "MacBatch.cs").read_text()
    call = "S8Unity.Benchmark.Run();"
    if template.count(call) != 1 or template.count('"-s8"') != 1:
        raise ValueError("control wrapper changed; review the feature adapter before preparation")
    entry = "AraExtended.ExtendedBenchmark.Run();" if cases.extended(args) else "AraFeatures.QueryBenchmark.Run();"
    derived = recovery.adapt_wrapper(template).replace(call, entry).replace('"-s8"', '"-ara-feature"')
    derived = derived.replace("missing S8 arguments", "missing feature arguments")
    source = HERE / ("extended/unity/Assets" if cases.extended(args) else "unity/Assets")
    inputs.sync_assets(source, root / "Assets",
                       ["Editor/MacBatch.cs", "Editor/Features.Editor.asmdef", "Editor/PreferenceRecovery.cs"])
    editor = root / "Assets/Editor"
    editor.mkdir(parents=True, exist_ok=True)
    inputs.write_if_changed(editor / "MacBatch.cs", derived.encode())
    inputs.write_if_changed(editor / "PreferenceRecovery.cs", (HERE / "editor/PreferenceRecovery.cs").read_bytes())
    assembly_name = "AraExtendedFixture" if cases.extended(args) else "AraFeatureFixture"
    assembly = {"name": assembly_name + ".Editor", "references": [assembly_name, "Unity.Burst"], "includePlatforms": ["Editor"]}
    inputs.write_if_changed(editor / "Features.Editor.asmdef", (json.dumps(assembly, indent=2) + "\n").encode())
    inputs.write_if_changed(root / "Packages/manifest.json", (json.dumps({"dependencies": BASE.PACKAGES}, indent=2) + "\n").encode())
    settings = root / "ProjectSettings"
    settings.mkdir(parents=True, exist_ok=True)
    version_path = settings / "ProjectVersion.txt"
    if not version_path.exists() or "m_EditorVersion: 6000.4.5f1" not in version_path.read_text().splitlines():
        inputs.write_if_changed(version_path, b"m_EditorVersion: 6000.4.5f1\n")
    BASE.write_json(marker, {"fixture": "extended-v1" if cases.extended(args) else "query_filter-v1", "editor": str(BASE.EDITOR),
                    "repository_root": str(ROOT),
                    "requested_packages": BASE.PACKAGES, "input_manifest_version": 1,
                    "template_inputs": template_inputs(), "prepared_inputs": inputs.prepared(root),
                    "wrapper_template_sha256": hashlib.sha256(template.encode()).hexdigest(),
                    "prepared_wrapper_sha256": hashlib.sha256(derived.encode()).hexdigest()})
    recovery.register(root)
    print(json.dumps({"project": str(root), "marker": str(marker), "launched": False}))


def command(args, engine):
    prepared_manifest(args)
    result, dump, log = paths(args, engine)
    environment = dict(os.environ)
    if engine == "bevy":
        binary = BASE.scratch_path(args.rust_binary)
        workers = cases.workers(args)
        input_argument = str(config_path(args)) if cases.extended(args) else str(args.archetypes)
        argv = ["/usr/bin/nice", "-n", "10", str(binary), cases.name(args), str(args.count), str(workers),
                str(args.warmup), str(args.frames), str(result), str(dump), input_argument, str(args.batch)]
    else:
        root = BASE.scratch_path(args.project)
        environment.update(UPM_CACHE_ROOT=str(root / "upm-cache"),
                           UPM_GIT_LFS_CACHE_PATH=str(root / "upm-cache/git-lfs"),
                           TMPDIR=str(ROOT / ("target/perf-scale/ufx-t" if cases.extended(args) else "target/perf-scale/uf-t")))
        argv = ["/usr/bin/nice", "-n", "10", str(BASE.EDITOR), "-batchmode", "-nographics",
                "-projectPath", str(root), "-job-worker-count", "14", "-executeMethod", "AraIteration.MacBatch.Run",
                "-logFile", str(log), "-ara-feature", cases.name(args), str(args.count), str(args.runners),
                str(args.warmup), str(args.frames), str(result), str(dump),
                str(config_path(args)) if cases.extended(args) else str(args.archetypes), str(args.batch)]
    return argv, environment, result, dump, log


def binary_name(args=None):
    return "ara-ecs-extended-fixture" if args is not None and cases.extended(args) else "ara-ecs-feature-fixture"


def rust_target(args=None):
    return EXTENDED_RUST_TARGET if args is not None and cases.extended(args) else RUST_TARGET


def config_path(args):
    return BASE.scratch_path(args.project) / "inputs" / args.run / (cases.name(args) + "_" + cases.variant(args) + ".json")


def recovery_temp(args):
    return ROOT / ("target/perf-scale/ufx-t" if cases.extended(args) else "target/perf-scale/uf-t")


def build_command(args=None):
    return {"command": ["/usr/bin/nice", "-n", "10", "cargo", "build", "--locked", "--release", "--bin", binary_name(args), "--manifest-path", str(HERE / "rust/Cargo.toml")],
            "environment": {"CARGO_TARGET_DIR": str(rust_target(args)), "CARGO_BUILD_JOBS": "2",
                            "RUSTFLAGS": "-C target-cpu=native", "CARGO_PROFILE_RELEASE_LTO": "thin",
                            "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "1", "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3",
                            "CARGO_PROFILE_RELEASE_DEBUG": "0", "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS": "false"}}


def resources(strict=True):
    """Timed engines require quiet tools; smoke/build lanes retain observed context."""
    busy = []
    rows = subprocess.check_output(["ps", "-axo", "pid=,pcpu=,comm="], text=True)
    for line in rows.splitlines():
        parts = line.split(None, 2)
        if len(parts) != 3:
            continue
        pid, cpu, command = parts
        build = any(name in command.lower() for name in ["rustc", "/cargo", "clippy-driver", "/dotnet", "vbcscompiler"])
        test = "/target/" in command and any(name in command for name in ["/deps/", "/examples/"])
        if float(cpu) > 1 and (build or test):
            argv = subprocess.run(["ps", "-p", pid, "-o", "command="], capture_output=True, text=True).stdout.strip()
            busy.append({"pid": int(pid), "pcpu": float(cpu), "command": argv})
    if busy and strict:
        raise RuntimeError("competing tool/test work observed; no valid timing claim: " + json.dumps(busy))
    return {"loadavg": list(os.getloadavg()), "busy_tools": busy, "strict_timing_guard": strict,
            "power": subprocess.check_output(["pmset", "-g", "batt"], text=True).strip()}
