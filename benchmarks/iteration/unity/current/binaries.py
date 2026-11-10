"""Require successful root build provenance, then use immutable content snapshots."""
import hashlib
import json
from pathlib import Path

import s8_project as project


def checked_sources(record):
    hashes = record.get("source_sha256")
    if not isinstance(hashes, dict) or not hashes: raise ValueError("build lacks source_sha256 manifest")
    required = {"Cargo.lock", "Cargo.toml", "crates/ara-sim/benches/ecs.rs"}
    required.update(str(path.relative_to(project.ROOT)) for path in (project.ROOT / "crates/ara-sim/benches/support").glob("*.rs"))
    if not required <= hashes.keys(): raise ValueError("build source manifest misses benchmark kernels/root lock")
    for name, digest in hashes.items():
        path = Path(name)
        if path.is_absolute() or ".." in path.parts: raise ValueError("invalid build source path")
        if project.sha(project.ROOT / path) != digest: raise ValueError("current source differs from build: " + name)


def snapshot(args):
    if args.build_record is None: raise ValueError("--build-record required for exact current compiled provenance")
    path = project.BASE.scratch_path(args.build_record)
    raw = path.read_bytes(); record = json.loads(raw)
    if record.get("exit_code") != 0 or not record.get("command") or not record.get("rustc"):
        raise ValueError("successful build command/compiler provenance required")
    checked_sources(record)
    binary_path = args.ara_binary or record.get("immutable_binary") or record.get("executable")
    if not binary_path: raise ValueError("--ara-binary or provenance executable required")
    binary = project.BASE.scratch_path(Path(binary_path))
    data = binary.read_bytes(); digest = hashlib.sha256(data).hexdigest()
    if digest != record.get("executable_sha256"): raise ValueError("binary differs from successful build record")
    folder = project.ROOT / "target/perf-scale/s8-current-binaries" / digest
    folder.mkdir(parents=True, exist_ok=True)
    archive = folder / "ecs"
    if not archive.exists():
        with archive.open("xb") as output: output.write(data)
        archive.chmod(0o755)
    if project.sha(archive) != digest: raise ValueError("immutable S8 binary archive corrupt")
    provenance = folder / (hashlib.sha256(raw).hexdigest() + ".build.json")
    if not provenance.exists():
        with provenance.open("xb") as output: output.write(raw)
    if provenance.read_bytes() != raw: raise ValueError("immutable build record corrupt")
    return archive, {"executable_sha256": digest, "snapshot": str(archive),
                     "build_provenance": str(provenance), "build_provenance_sha256": project.sha(provenance),
                     "source_sha256": record["source_sha256"]}
