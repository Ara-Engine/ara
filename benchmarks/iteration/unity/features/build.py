"""Explicit, logged builds and immutable executable snapshots for feature runs."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import signal
import subprocess
import time

import project


def locked_packages(path):
    """Read only Cargo.lock's simple quoted package headers, without a Python dependency."""
    packages = []
    for block in path.read_text().split("\n[[package]]\n")[1:]:
        fields = dict(re.findall(r'^(name|version|source) = "([^"\n]+)"$', block, re.MULTILINE))
        if "name" not in fields or "version" not in fields:
            raise ValueError("unexpected Cargo.lock package header")
        packages.append(fields)
    if not packages: raise ValueError("Cargo.lock contains no package headers")
    return packages


def source_hashes(args=None):
    source = project.HERE / "rust/src"
    files = list((source / "extended").rglob("*.rs")) if args is not None and project.cases.extended(args) else list(source.glob("*.rs"))
    files += [project.HERE / "rust/Cargo.toml", project.HERE / "rust/Cargo.lock"]
    files += list((project.ROOT / "crates/ara-json/src").rglob("*.rs")) + [project.ROOT / "crates/ara-json/Cargo.toml"]
    return {str(path.relative_to(project.ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in files}


def checked_build_settings(record, args=None):
    """Bind the requested feature/profile to compiled provenance before execution."""
    expected = project.build_command(args)
    if record.get("command") != expected["command"] or record.get("environment") != expected["environment"]:
        raise ValueError("Rust build command/environment/features differ from the requested buffer kernel")
    kernel = project.buffer_kernel(args)
    requested = record.get("buffer_kernel_requested")
    if requested is not None and requested != kernel:
        raise ValueError("Rust build records another requested buffer kernel")
    contract = record.get("buffer_kernel_contract_version")
    legacy = kernel == "baseline" and requested is None and contract is None
    if not legacy and (requested != kernel or contract != 1):
        raise ValueError("buffer kernel lacks matching versioned feature provenance")
    label = {"baseline": "baseline-row-mut-scalar", "contiguous": "contiguous-scalar"}.get(kernel)
    cfg = record.get("rustc_target_cfg")
    if kernel == "simd":
        command = record.get("rustc_target_cfg_command", [])
        flags = ["--print", "cfg", *shlex.split(expected["environment"]["RUSTFLAGS"])]
        if record.get("cargo_build_target"):
            flags += ["--target", record["cargo_build_target"]]
        if not isinstance(cfg, list) or not all(isinstance(row, str) for row in cfg) or len(command) < 2 or command[1:] != flags:
            raise ValueError("SIMD buffer kernel lacks matching compiler target-feature proof")
        architectures = [row for row in cfg if row.startswith('target_arch="')]
        if len(architectures) != 1:
            raise ValueError("SIMD compiler target architecture is ambiguous")
        neon = 'target_arch="aarch64"' in cfg and 'target_feature="neon"' in cfg
        label = "contiguous-neon4" if neon else "contiguous-scalar-fallback"
    return {"requested": kernel, "expected_reported_label": label,
            "legacy_metadata_allowed": legacy,
            "rustc_target_cfg": cfg}


def checked_result_kernel(data, executable, args):
    """A SIMD feature may report scalar fallback; never relabel it as NEON."""
    if project.cases.name(args) not in ("buffer_fixed", "buffer_resize"):
        return None
    proof = executable["buffer_kernel"]
    reported = data.get("buffer_kernel")
    if reported is None and proof["legacy_metadata_allowed"]:
        return None
    if reported != proof["expected_reported_label"]:
        raise ValueError("Rust reported buffer kernel differs from compiled feature/architecture proof")
    return reported


def snapshot(binary, args=None):
    record_path = project.rust_target(args) / "build-provenance.json"
    provenance_raw = record_path.read_bytes()
    record = json.loads(provenance_raw)
    kernel = checked_build_settings(record, args)
    if record["exit_code"] != 0 or record["source_sha256"] != source_hashes(args):
        raise ValueError("Rust fixture source differs from the successful build; rebuild in a CPU slot")
    binary = project.BASE.scratch_path(binary)
    data = binary.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != record["executable_sha256"]:
        raise ValueError("Rust executable differs from recorded build")
    archive = project.ROOT / "target/perf-scale/feature-binaries" / digest / project.binary_name(args)
    archive.parent.mkdir(parents=True, exist_ok=True)
    if not archive.exists():
        with archive.open("xb") as output:
            output.write(data)
        archive.chmod(0o755)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
        raise ValueError("immutable executable snapshot is corrupt")
    provenance_digest = hashlib.sha256(provenance_raw).hexdigest()
    provenance = archive.parent / (provenance_digest + ".build.json")
    if not provenance.exists():
        with provenance.open("xb") as output: output.write(provenance_raw)
    if provenance.read_bytes() != provenance_raw:
        raise ValueError("immutable build provenance snapshot is corrupt")
    return archive, {"executable_sha256": digest, "snapshot": str(archive),
                     "build_provenance": str(provenance), "original_build_provenance": str(record_path),
                     "build_provenance_sha256": provenance_digest, "buffer_kernel": kernel}


def build(args):
    if project.BASE.active_editors():
        raise ValueError("a Unity Editor is already running")
    resources_before = project.resources(strict=False)
    lock = locked_packages(project.HERE / "rust/Cargo.lock")
    dependencies = {entry["name"]: entry["version"] for entry in lock if entry["name"].startswith("bevy_")}
    root_bevy = {(entry["name"], entry["version"]) for entry in locked_packages(project.ROOT / "Cargo.lock") if entry["name"].startswith("bevy_")}
    if any((name, version) not in root_bevy for name, version in dependencies.items()):
        raise ValueError("fixture Bevy family differs from the production lock; align explicitly before building")
    settings = project.build_command(args)
    environment = dict(os.environ)
    removed = [key for key in ["CARGO_ENCODED_RUSTFLAGS"] if key in environment]
    for key in removed:
        del environment[key]
    environment.update(settings["environment"])
    output = project.rust_target(args) / "builds" / str(time.time_ns())
    output.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    record = {"command": settings["command"], "environment": settings["environment"],
              "source_sha256": source_hashes(args), "host": platform.platform(),
              "bevy_locked_versions": dependencies,
              "locked_packages": [{"name": entry["name"], "version": entry["version"], "source": entry.get("source", "local-path")} for entry in lock],
              "architecture": platform.machine(), "cargo_path": shutil.which("cargo"),
              "removed_environment_keys": removed,
              "resource_context_before": resources_before, "timings_accepted": False,
              "rustc": subprocess.check_output([environment.get("RUSTC", "rustc"), "-Vv"], env=environment, text=True),
              "git_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=project.ROOT, text=True).strip()}
    if project.cases.extended(args):
        record.update(buffer_kernel_requested=project.buffer_kernel(args), buffer_kernel_contract_version=1)
    if project.buffer_kernel(args) == "simd":
        cfg_command = [environment.get("RUSTC", "rustc"), "--print", "cfg", *shlex.split(settings["environment"]["RUSTFLAGS"])]
        target = environment.get("CARGO_BUILD_TARGET")
        if target:
            cfg_command += ["--target", target]
        record.update(cargo_build_target=target, rustc_target_cfg_command=cfg_command,
                      rustc_target_cfg=subprocess.check_output(cfg_command, env=environment, text=True).splitlines())
    with (output / "build.log").open("w") as log:
        with subprocess.Popen(settings["command"], env=environment, cwd=project.ROOT,
                              stdout=log, stderr=subprocess.STDOUT, start_new_session=True) as child:
            try:
                status = child.wait(timeout=args.timeout)
            finally:
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGTERM)
                    try: child.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        os.killpg(child.pid, signal.SIGKILL)
                        child.wait()
    record.update(exit_code=status, wall_seconds=time.monotonic() - started,
                  resource_context_after=project.resources(strict=False))
    if status == 0:
        if record["source_sha256"] != source_hashes(args):
            raise RuntimeError("fixture source changed during build")
        binary = project.rust_target(args) / "release" / project.binary_name(args)
        record["executable_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        lock = project.HERE / "rust/Cargo.lock"
        record["cargo_lock_sha256"] = hashlib.sha256(lock.read_bytes()).hexdigest()
        project.BASE.write_json(project.rust_target(args) / "build-provenance.json", record)
        snapshot(binary, args)
    project.BASE.write_json(output / "build-provenance.json", record)
    print(json.dumps({"exit_code": status, "log": str(output / "build.log"),
                      "executable_sha256": record.get("executable_sha256")}))
    if status:
        raise RuntimeError("focused Rust feature build failed")
