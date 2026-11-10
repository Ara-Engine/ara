"""Bind evidence and compare complete numeric hashes plus independent exact IDs."""
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess

import s8_project as project
import inputs
import recovery


def environment(result):
    path = Path(str(result) + ".environment.json")
    metadata = json.loads(path.read_text())
    if any(metadata.get(key) is not True for key in ("finally_restored", "measurement_completed", "burst_enabled")) or any(
            metadata.get(key) is not False for key in ("burst_safety_checks", "burst_force_safety_checks", "burst_debug")):
        raise ValueError("effective Burst/restoration controls unproven")
    return metadata, path


def check_samples(row, frames):
    samples = row["ms"]
    if len(samples) != frames or any(not math.isfinite(x) or x < 0 for x in samples):
        raise ValueError("invalid timing samples")
    ordered = sorted(samples)
    for key, fraction in (("p50_ms", .50), ("p95_ms", .95)):
        if not math.isclose(row[key], ordered[math.ceil(frames * fraction) - 1], rel_tol=1e-12, abs_tol=1e-12):
            raise ValueError("reported percentile differs from actual samples")


def identities(data, args):
    if len(data) != args.count * 4: raise ValueError("exact ID sidecar byte count differs")
    steps = args.warmup + args.frames
    for slot, (identifier,) in enumerate(struct.iter_unpack("<i", data)):
        expected = slot
        if args.workload == "w4" and steps > slot // 10_000:
            group = slot // 10_000
            last = steps - 1
            frame = last - (last - group) % 10
            expected = 100_000 + frame * 10_000 + slot % 10_000
        if identifier != expected: raise ValueError("full ID/epoch sidecar differs from contract")


def verify(args):
    paths = {engine: project.paths(args, engine) for engine in ("ara", "unity")}
    rows, launches = {}, {}
    for engine, (result, _, _) in paths.items():
        launch = json.loads(result.with_suffix(".launch.json").read_text())
        if launch["result_sha256"] != project.sha(result): raise ValueError("result bytes changed since launch")
        if launch["config"] != project.config(args): raise ValueError("launch config differs")
        if launch["config_sha256"] != hashlib.sha256(json.dumps(launch["config"], sort_keys=True).encode()).hexdigest():
            raise ValueError("launch config hash differs")
        if project.sha(project.BASE.scratch_path(Path(launch["config_path"]))) != launch["config_sha256"]:
            raise ValueError("on-disk config changed since launch")
        prepared = launch["prepared_project"]
        if inputs.manifest_digest(prepared["input_manifest"]) != prepared["input_manifest_sha256"] or not launch["prepared_inputs_unchanged_after_run"]:
            raise ValueError("prepared manifest proof differs")
        row = json.loads(result.read_text())
        for key in ("workload", "count", "warmup", "frames"):
            if row[key] != getattr(args, key): raise ValueError("result config differs: " + key)
        check_samples(row, args.frames)
        expected_lane = "correctness-smoke" if args.smoke else "timed"
        if launch["execution_lane"] != expected_lane: raise ValueError("smoke cannot become timed evidence")
        if not args.smoke and not launch["strict_timing_guard_enforced"]: raise ValueError("timing guard was not enforced")
        rows[engine], launches[engine] = row, launch
    executable = launches["ara"]["executable"]
    if project.sha(project.BASE.scratch_path(Path(executable["snapshot"]))) != executable["executable_sha256"]:
        raise ValueError("immutable Ara binary changed")
    build_path = project.BASE.scratch_path(Path(executable["build_provenance"]))
    if project.sha(build_path) != executable["build_provenance_sha256"]:
        raise ValueError("immutable build provenance changed")
    build = json.loads(build_path.read_text())
    if build["source_sha256"] != executable["source_sha256"] or build["executable_sha256"] != executable["executable_sha256"]:
        raise ValueError("build manifest differs from launched binary/sources")
    if rows["ara"]["variant"] != args.variant or rows["ara"]["batch"] != args.batch or rows["ara"]["threads"] != project.budget(args)["ara"]["argument_threads"]:
        raise ValueError("Ara variant/batch/worker setting differs")
    if rows["ara"].get("stable_ids_asserted") is not True: raise ValueError("current Ara exact ID/component checks required")
    if rows["unity"]["threads"] != args.runners or rows["unity"]["job_workers"] != args.runners - 1:
        raise ValueError("Unity actual worker count differs")
    metadata, metadata_path = environment(paths["unity"][0])
    unity_launch = launches["unity"]
    if metadata["job_workers"] != args.runners - 1 or project.sha(metadata_path) != unity_launch["environment_sha256"]:
        raise ValueError("Unity worker/environment proof differs")
    proof = recovery.successful(project.BASE.scratch_path(args.project), paths["unity"][0], metadata)
    if proof != unity_launch["initial_preference_checkpoint"]: raise ValueError("checkpoint/receipt changed")
    dump = paths["unity"][1]; ids = Path(str(dump) + ".ids")
    if dump.stat().st_size != args.count * 48: raise ValueError("numeric state dump byte count differs")
    if project.sha(dump) != unity_launch["numeric_dump_sha256"] or project.sha(ids) != unity_launch["ids_sha256"]:
        raise ValueError("state dump changed since launch")
    identities(ids.read_bytes(), args)
    tool = project.BASE.scratch_path(args.project) / "tools/state-hash"
    library, flags = project.BASE.cached_tool("blake3", "dump_hash.rs", tool)
    digest = subprocess.check_output(["/usr/bin/nice", "-n", "10", str(tool), str(dump)], text=True).strip()
    if digest != rows["ara"]["state_hash"]: raise ValueError("full numeric state hash differs; timings invalid")
    controls = all(launch["resource_guards_passed"] and launch["timings_accepted"] for launch in launches.values())
    same_power = len({launch["before"]["power"].splitlines()[0] for launch in launches.values()}) == 1
    same_inputs = len({launch["prepared_project"]["input_manifest_sha256"] for launch in launches.values()}) == 1
    comparable = not args.smoke and controls and same_power and same_inputs
    gain = 1 - rows["ara"]["p50_ms"] / rows["unity"]["p50_ms"]
    tail = rows["ara"]["p95_ms"] / rows["unity"]["p95_ms"] - 1
    report = {"config": project.config(args), "quality_passed": True, "timings_accepted": comparable,
        "timing_comparable": comparable, "numeric_state_blake3": digest,
        "state_proof": "Unity complete 12xf32 dump BLAKE3 equals Ara complete post-loop numeric hash; independent exact IDs/components asserted by both; Ara does not export numeric bytes",
        "unity_exact_ids_sha256": project.sha(ids), "numeric_dump_bytes": dump.stat().st_size,
        "hash_library_sha256": project.sha(library), "hash_helper_link_flags": flags,
        "result_sha256": {engine: project.sha(path[0]) for engine, path in paths.items()},
        "initial_preference_checkpoint": proof, "worker_ceilings": project.budget(args),
        "power_profile_matched": same_power, "prepared_inputs_matched": same_inputs,
        "ara_p50_fraction_lower": gain if comparable else None,
        "ara_p95_fraction_change": tail if comparable else None,
        "target": {"p50_fraction_lower_at_least": .20, "p95_regression_fraction_at_most": .05},
        "target_met": comparable and gain >= .20 and tail <= .05,
        "scope": "Mac CPU-only Rust native vs Unity Editor Burst; no player/Windows/GPU/Unreal claim",
        "verification_source_sha256": {str(path.relative_to(project.ROOT)): project.sha(path) for path in verification_sources()}}
    project.BASE.write_json(paths["unity"][0].with_suffix(".validation.json"), report)
    print(json.dumps(report))


def verification_sources():
    return [Path(__file__), project.HERE / "s8_project.py", project.HERE / "run.py", project.features.HERE / "recovery.py"]
