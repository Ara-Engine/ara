"""Current S8 comparisons: explicit prepare, controlled fresh runs, then validation."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time

import s8_project as project
import binaries
import checks
import inputs
import monitor
import recovery
import recover


def run(args, engine):
    if project.BASE.active_editors(): raise ValueError("a Unity Editor is running; leave it untouched")
    if engine == "unity": recovery.ensure_clear(args.project)
    binary, executable = binaries.snapshot(args) if engine == "ara" else (None, None)
    argv, env, result, dump, log = project.command(args, engine, binary)
    if any(path.exists() for path in (result, dump, log, result.with_suffix(".launch.json"))):
        raise ValueError("case outputs exist; use a fresh --run")
    result.parent.mkdir(parents=True, exist_ok=True)
    if "TMPDIR" in env: Path(env["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    config = project.config(args)
    config_bytes = json.dumps(config, sort_keys=True).encode()
    config_path = result.with_suffix(".input.json")
    with config_path.open("xb") as output: output.write(config_bytes)
    prepared = project.prepared(args)
    before = project.features.resources(strict=not args.smoke)
    started = time.monotonic()
    with log.with_suffix(".launcher.log").open("x") as output:
        with subprocess.Popen(argv, env=env, cwd=project.ROOT, stdout=output, stderr=subprocess.STDOUT) as child:
            try:
                try: status, observed = monitor.wait(child, args.timeout, project.features.resources)
                finally:
                    if child.poll() is None:
                        child.terminate()
                        try: child.wait(timeout=10)
                        except subprocess.TimeoutExpired: child.kill(); child.wait()
            except BaseException:
                if engine == "unity": recovery.failed(args.project, result, child.poll())
                raise
    if status or not result.exists():
        if engine == "unity": recovery.failed(args.project, result, status)
        raise RuntimeError(f"fixture failed ({status}); retain {log}, no automatic retry")
    proof, environment_path = None, None
    if engine == "unity":
        metadata, environment_path = checks.environment(result)
        proof = recovery.successful(args.project, result, metadata)
        if metadata["job_workers"] != args.runners - 1: raise ValueError("unexpected actual Unity worker count")
        checks.identities(Path(str(dump) + ".ids").read_bytes(), args)
    after = project.features.resources(strict=False)
    try:
        if project.prepared(args)["marker_sha256"] != prepared["marker_sha256"]: raise ValueError("marker changed")
    except ValueError as error:
        actual = inputs.prepared(project.BASE.scratch_path(args.project))
        project.BASE.write_json(result.with_suffix(".unaccepted.json"), {"timings_accepted": False,
            "initial_import_bootstrap_outputs": inputs.bootstrap_changes(prepared["input_manifest"], actual),
            "prepared_project": prepared, "observed_prepared_inputs": actual, "error": str(error)})
        raise RuntimeError("input drift/bootstrap: reprepare then fresh smoke; timings unaccepted") from error
    if config_path.read_bytes() != config_bytes: raise ValueError("case config changed during run")
    if executable is not None:
        record = json.loads(Path(executable["build_provenance"]).read_text())
        binaries.checked_sources(record)
    quiet = not before["busy_tools"] and not after["busy_tools"] and monitor.quiet(observed)
    power_matches = monitor.power_stable(before, observed, after)
    accepted = not args.smoke and quiet and power_matches
    launch = {"command": argv, "nice": 10, "wall_seconds": time.monotonic() - started,
        "config": config, "config_path": str(config_path), "config_sha256": hashlib.sha256(config_bytes).hexdigest(),
        "execution_lane": "correctness-smoke" if args.smoke else "timed", "timings_accepted": accepted,
        "strict_timing_guard_enforced": not args.smoke, "resource_guards_passed": quiet,
        "before": before, "after": after, "resource_observations_during_run": observed,
        "resource_sample_interval_seconds": 2, "power_source_unchanged": power_matches,
        "prepared_project": prepared, "prepared_inputs_unchanged_after_run": True,
        "executable": executable, "initial_preference_checkpoint": proof,
        "worker_ceilings": project.budget(args), "result_sha256": project.sha(result),
        "environment_sha256": project.sha(environment_path) if environment_path else None,
        "numeric_dump_sha256": project.sha(dump) if engine == "unity" else None,
        "ids_sha256": project.sha(Path(str(dump) + ".ids")) if engine == "unity" else None}
    project.BASE.write_json(result.with_suffix(".launch.json"), launch)
    if not args.smoke and not accepted: raise RuntimeError("resource/power guard failed; raw outputs retained, no timing claim")
    print(json.dumps({"result": str(result), "execution_lane": launch["execution_lane"], "timings_accepted": accepted}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "command", "run-ara", "run-unity", "verify", "recover-unity"))
    parser.add_argument("--project", type=Path, default=project.PROJECT)
    parser.add_argument("--workload", choices=project.COUNTS, default="w1")
    parser.add_argument("--count", type=int)
    parser.add_argument("--runners", type=int, choices=(1, 15), default=1)
    parser.add_argument("--warmup", type=int)
    parser.add_argument("--frames", type=int, default=300)
    parser.add_argument("--batch", type=int, default=4096)
    parser.add_argument("--variant", choices=("baseline", "tuned", "simd"), default="simd")
    parser.add_argument("--run", default="current-01")
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--build-record", type=Path)
    parser.add_argument("--ara-binary", type=Path)
    parser.add_argument("--checkpoint", type=Path)
    parser.add_argument("--engine", choices=("ara", "unity"), default="unity")
    args = parser.parse_args()
    if args.count is None: args.count = project.COUNTS[args.workload]
    if args.warmup is None: args.warmup = 3000 if args.workload == "w3" else 120
    if args.count < 64 or args.frames < 1 or args.warmup < 0 or args.batch < 1 or args.timeout < 1:
        parser.error("invalid entity/step/batch/timeout parameters")
    if args.count > 2**31 - 1 or args.warmup + args.frames > 2**31 - 1:
        parser.error("Unity count/combined step limit exceeded")
    if args.workload == "w4" and (args.count != 100_000 or args.frames + args.warmup > 200_000):
        parser.error("W4 requires100000 entities and <=200000 steps for signed identity bounds")
    if args.smoke and args.warmup + args.frames > 10: parser.error("smoke permits at most10 steps")
    if not args.run or args.run in (".", "..") or any(char in args.run for char in "/\\"):
        parser.error("--run must be one fresh folder name")
    if args.action == "prepare": project.prepare(args)
    elif args.action == "recover-unity": recover.run(args, project)
    elif args.action == "verify": checks.verify(args)
    elif args.action == "command":
        binary, _ = binaries.snapshot(args) if args.engine == "ara" else (None, None)
        command, _, _, _, _ = project.command(args, args.engine, binary)
        print(json.dumps({"command": command, "worker_ceilings": project.budget(args)}))
    else: run(args, "ara" if args.action == "run-ara" else "unity")


if __name__ == "__main__": main()
