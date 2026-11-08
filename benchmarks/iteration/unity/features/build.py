"""Explicit, logged builds and immutable executable snapshots for feature runs."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
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


def snapshot(binary, args=None):
    record_path = project.rust_target(args) / "build-provenance.json"
    record = json.loads(record_path.read_text())
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
    return archive, {"executable_sha256": digest, "snapshot": str(archive),
                     "build_provenance": str(record_path),
                     "build_provenance_sha256": hashlib.sha256(record_path.read_bytes()).hexdigest()}


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
