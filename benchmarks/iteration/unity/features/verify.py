"""Independent binary validation, separate from timed engine execution."""
import hashlib
import json
from pathlib import Path
import struct
import subprocess

import project
import inputs
import cases
import reference as scalar
import recovery


def checked_result(path):
    """Bind parsed timings to the exact result bytes recorded by their launch."""
    raw = path.read_bytes()
    launch = json.loads(path.with_suffix(".launch.json").read_text())
    if hashlib.sha256(raw).hexdigest() != launch["result_sha256"]:
        raise ValueError("result changed since the accepted launch; retain original evidence")
    return json.loads(raw), launch


def checked_lane(launches, args):
    expected = project.execution_lane(args)
    timed = expected == "timed"
    for launch in launches.values():
        if (launch.get("execution_lane") != expected or launch.get("timings_accepted") is not timed
                or launch.get("strict_timing_guard_enforced") is not timed):
            raise ValueError("launch lane differs; smoke/unaccepted evidence cannot become timed evidence")
    return expected


def checked_case_input(launch, args):
    """Bind the immutable case config and its on-disk bytes to this exact launch."""
    recorded = launch.get("case_input")
    expected = cases.encoded_config(args)
    if not recorded or recorded["config"] != cases.config(args) or recorded["sha256"] != hashlib.sha256(expected).hexdigest():
        raise ValueError("case input is not bound to the exact launch")
    if hashlib.sha256(project.BASE.scratch_path(Path(recorded["path"])).read_bytes()).hexdigest() != recorded["sha256"]:
        raise ValueError("case input changed since launch")


def verification_sources():
    """Hash the executing verifier modules separately from prepared engine inputs."""
    here = Path(__file__).resolve().parent
    root = here.parents[3]
    paths = [here / name for name in ["verify.py", "reference.py", "cases.py", "inputs.py", "project.py"]]
    paths.append(here.parent / "bench.py")
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}


def audit(data, args):
    if cases.extended(args):
        return audit_extended(data, args)
    if len(data) != 24 + args.count * 36:
        raise ValueError("query-filter state byte count differs")
    header = struct.unpack_from("<8sIIII", data)
    if header != (b"ARAECSF1", 1, 1, args.count, args.warmup + args.frames):
        raise ValueError("feature header differs from requested case")
    for identifier in range(args.count):
        row = struct.unpack_from("<9I", data, 24 + identifier * 36)
        tags = identifier & (args.archetypes - 1)
        updates = args.warmup + args.frames if tags & 1 and not tags & 2 else 0
        if row[0] != identifier or row[1] != tags or row[7] != updates:
            raise ValueError("stable ID, tag presence or selected update count differs")
        if row[3:7] != (0, 0, 0, 0) or row[8] != 0:
            raise ValueError("inactive fields are not canonical zeroes")


def audit_extended(data, args):
    """Decode every row/word and reject identity, presence, epoch or operation loss."""
    name = cases.name(args)
    steps = args.warmup + args.frames
    if len(data) < 24 or struct.unpack_from("<8sIIII", data) != (b"ARAECSF1", 1, cases.NAMES.index(name) + 1, args.count, steps):
        raise ValueError("extended feature header differs")
    offset = 24
    for identifier in range(args.count):
        if offset + 36 > len(data): raise ValueError("truncated feature row")
        row = struct.unpack_from("<9I", data, offset)
        offset += 36
        tags, epoch, updates, length = 0, 0, steps, 0
        if name == "dirty_epoch":
            chosen = [tick for tick in range(steps) if scalar.selected(identifier, tick, args.count, args.percent, args.distribution)]
            epoch = chosen[-1] + 1 if chosen else 0
            updates = len(chosen)
        elif name == "archetype_churn":
            updates = (steps + 1) // 2 if identifier % 100 < args.percent else 0
            tags = 8 if steps and (steps - 1) % 2 == 0 and identifier % 100 < args.percent else 0
        elif name == "buffer_fixed": length = args.length
        elif name == "buffer_resize": length = 64 if steps and (steps - 1) % 2 == 0 and identifier % 100 < args.percent else 4
        if (row[0], row[1], row[3], row[7], row[8]) != (identifier, tags, epoch, updates, length):
            raise ValueError("extended ID/tag/epoch/update/buffer-length audit differs")
        if name != "dependency_dag" and row[4:7] != (0, 0, 0): raise ValueError("inactive DAG columns are nonzero")
        offset += length * 4
        if offset > len(data): raise ValueError("truncated buffer payload")
    if offset != len(data): raise ValueError("trailing feature state bytes")
    if project.execution_lane(args) == "correctness-smoke":
        rows = scalar.initial(name, args.count, 8, args.length)
        for tick in range(steps): scalar.step(rows, name, tick, args.percent, args.distribution)
        if data != scalar.encode(rows, name, steps): raise ValueError("extended engine state differs from independent integer oracle")


def verify(args):
    paths = {engine: project.paths(args, engine) for engine in ["bevy", "unity"]}
    checked = {engine: checked_result(path[0]) for engine, path in paths.items()}
    results = {engine: result for engine, (result, _) in checked.items()}
    launches = {engine: launch for engine, (_, launch) in checked.items()}
    lane = checked_lane(launches, args)
    if lane == "correctness-smoke" and args.reference is None:
        raise ValueError("correctness smoke requires an independent scalar reference")
    states = {engine: path[1].read_bytes() for engine, path in paths.items()}
    metadata = json.loads(Path(str(paths["unity"][0]) + ".environment.json").read_text())
    if (not metadata.get("finally_restored") or not metadata.get("measurement_completed")
            or not metadata.get("burst_enabled") or metadata.get("burst_safety_checks") is not False
            or metadata.get("burst_force_safety_checks") is not False or metadata.get("burst_debug") is not False):
        raise ValueError("controlled Unity options/restoration are not proven")
    for engine, result in results.items():
        for key, expected in [("count", args.count), ("archetypes", 0 if cases.extended(args) else args.archetypes),
                              ("warmup", args.warmup), ("frames", args.frames), ("batch", args.batch)]:
            if result[key] != expected:
                raise ValueError(f"{engine} has different {key}")
        if cases.extended(args):
            if result["workload"] != cases.name(args) or result["config"] != cases.config(args):
                raise ValueError("extended workload/config differs")
            checked_case_input(launches[engine], args)
        if not result["full_ids_tags_updates_checked"] or result["max_kernel_runners"] != args.runners:
            raise ValueError("identity audit or runner budget differs")
        audit(states[engine], args)
    if cases.name(args) == "dirty_epoch":
        bevy_visits = results["bevy"]["dirty_diagnostics"]["candidate_logical_visits"]
        unity_candidates = results["unity"]["diagnostic_candidate_visits"]
        unity_logical = results["unity"]["diagnostic_logical_updates"]
        if len(bevy_visits) != args.warmup + args.frames or len(unity_candidates) != len(bevy_visits) or len(unity_logical) != len(bevy_visits):
            raise ValueError("missing dirty diagnostic steps")
        for tick, (record, candidate, logical) in enumerate(zip(bevy_visits, unity_candidates, unity_logical)):
            expected = sum(scalar.selected(identifier, tick, args.count, args.percent, args.distribution) for identifier in range(args.count))
            if record[0] != tick or record[2] != expected or logical != expected or not expected <= record[1] <= args.count or not expected <= candidate <= args.count:
                raise ValueError("real filter candidate/logical replay counts differ")
        if not results["bevy"]["dirty_diagnostics"]["separate_replay"]:
            raise ValueError("dirty diagnostics were not separated from throughput")
    if states["bevy"] != states["unity"]:
        raise ValueError("complete engine states differ; do not compare timings")
    tool = project.BASE.scratch_path(args.project) / "tools/feature-state-hash"
    library, link_flags = project.BASE.cached_tool("blake3", "dump_hash.rs", tool)
    digest = subprocess.check_output(["/usr/bin/nice", "-n", "10", str(tool), str(paths["bevy"][1])], text=True).strip()
    if digest != results["bevy"]["state_blake3"]:
        raise ValueError("Rust result digest differs from complete state bytes")
    reference_verified = False
    if args.reference is not None:
        reference = project.BASE.scratch_path(args.reference).read_bytes()
        audit(reference, args)
        if reference != states["bevy"]:
            raise ValueError("engines differ from independent scalar reference")
        reference_verified = True
    report = {"workload": cases.name(args), "count": args.count, "archetypes": 0 if cases.extended(args) else args.archetypes,
              "verification_source_sha256": verification_sources(),
              "execution_lane": lane, "quality_passed": True, "timings_accepted": lane == "timed",
              "full_state_byte_identical": True, "independent_scalar_reference_verified": reference_verified,
              "full_state_blake3": digest, "hash_library_sha256": hashlib.sha256(library.read_bytes()).hexdigest(),
              "hash_helper_link_flags": link_flags,
              "state_sha256": hashlib.sha256(states["bevy"]).hexdigest(),
              "state_bytes": len(states["bevy"]), "runner_budget": args.runners,
              "result_sha256": {engine: hashlib.sha256(path[0].read_bytes()).hexdigest() for engine, path in paths.items()}}
    if cases.extended(args):
        report["config"] = cases.config(args)
        report["worker_observations"] = {engine: result["worker_probe"] for engine, result in results.items()}
        report["storage_observations"] = {engine: result["layout"] for engine, result in results.items()}
        if cases.name(args) == "dirty_epoch":
            report["separate_dirty_replay"] = {"bevy_tick_candidate_logical": bevy_visits,
                "unity_candidates": unity_candidates, "unity_logical": unity_logical}
    for launch in launches.values():
        prepared = launch["prepared_project"]
        if (inputs.manifest_digest(prepared["input_manifest"]) != prepared["input_manifest_sha256"]
                or not launch["prepared_inputs_unchanged_after_run"]):
            raise ValueError("launch input-manifest proof differs or inputs changed during execution")
    unity_environment = Path(str(paths["unity"][0]) + ".environment.json").read_bytes()
    if hashlib.sha256(unity_environment).hexdigest() != launches["unity"]["environment_sha256"]:
        raise ValueError("Unity environment metadata changed after launch")
    checkpoint = launches["unity"].get("initial_preference_checkpoint")
    if checkpoint is not None:
        proof = recovery.successful(project.BASE.scratch_path(args.project), paths["unity"][0], metadata)
        if proof != checkpoint: raise ValueError("checkpoint/restoration receipt changed since launch")
        report["initial_preference_checkpoint"] = proof
    report["power_profile_matched"] = len({launch["before"]["power"].splitlines()[0] for launch in launches.values()}) == 1
    report["prepared_input_manifest_sha256"] = {engine: launch["prepared_project"]["input_manifest_sha256"]
                                                for engine, launch in launches.items()}
    report["prepared_input_manifests_match"] = len(set(report["prepared_input_manifest_sha256"].values())) == 1
    report["resource_guards_passed"] = all(launch["resource_guards_passed"] for launch in launches.values())
    report["timing_comparable_under_recorded_controls"] = (lane == "timed" and report["power_profile_matched"] and
        report["resource_guards_passed"] and report["prepared_input_manifests_match"])
    report["timing_comparable"] = report["timing_comparable_under_recorded_controls"]
    destination = paths["unity"][0].with_suffix(".validation.json")
    project.BASE.write_json(destination, report)
    print(json.dumps(report))
