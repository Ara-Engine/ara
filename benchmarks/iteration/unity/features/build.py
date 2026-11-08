"""Explicit, logged builds and immutable executable snapshots for feature runs."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import time

import project


def source_hashes():
    files = list((project.HERE / "rust/src").glob("*.rs")) + [project.HERE / "rust/Cargo.toml"]
    return {str(path.relative_to(project.ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in files}


def snapshot(binary):
    record_path = project.RUST_TARGET / "build-provenance.json"
    record = json.loads(record_path.read_text())
    if record["exit_code"] != 0 or record["source_sha256"] != source_hashes():
        raise ValueError("Rust fixture source differs from the successful build; rebuild in a CPU slot")
    binary = project.BASE.scratch_path(binary)
    data = binary.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != record["executable_sha256"]:
        raise ValueError("Rust executable differs from recorded build")
    archive = project.ROOT / "target/perf-scale/feature-binaries" / digest / "ara-ecs-feature-fixture"
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
    settings = project.build_command()
    environment = dict(os.environ)
    removed = [key for key in ["CARGO_ENCODED_RUSTFLAGS"] if key in environment]
    for key in removed:
        del environment[key]
    environment.update(settings["environment"])
    output = project.RUST_TARGET / "builds" / str(time.time_ns())
    output.mkdir(parents=True, exist_ok=False)
    started = time.monotonic()
    record = {"command": settings["command"], "environment": settings["environment"],
              "source_sha256": source_hashes(), "host": platform.platform(),
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
        if record["source_sha256"] != source_hashes():
            raise RuntimeError("fixture source changed during build")
        binary = project.RUST_TARGET / "release/ara-ecs-feature-fixture"
        record["executable_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        lock = project.HERE / "rust/Cargo.lock"
        record["cargo_lock_sha256"] = hashlib.sha256(lock.read_bytes()).hexdigest()
        project.BASE.write_json(project.RUST_TARGET / "build-provenance.json", record)
        snapshot(binary)
    project.BASE.write_json(output / "build-provenance.json", record)
    print(json.dumps({"exit_code": status, "log": str(output / "build.log"),
                      "executable_sha256": record.get("executable_sha256")}))
    if status:
        raise RuntimeError("focused Rust feature build failed")
