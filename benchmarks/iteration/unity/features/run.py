"""Prepare/run real query-filter ECS fixtures; no automatic engine/Cargo builds."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

sys.dont_write_bytecode = True
import project
import verify
import build
import inputs
import cases
import recovery


def run(args, engine):
    lane = project.execution_lane(args)
    timed = lane == "timed"
    if project.BASE.active_editors():
        raise ValueError("a Unity Editor is running; leave it untouched")
    if engine == "unity": recovery.ensure_clear(project.BASE.scratch_path(args.project))
    executable = None
    if engine == "bevy":
        args.rust_binary, executable = build.snapshot(args.rust_binary, args)
    argv, environment, result, dump, log = project.command(args, engine)
    if result.exists() or dump.exists() or log.exists():
        raise ValueError("case output exists; choose a fresh --run")
    result.parent.mkdir(parents=True, exist_ok=True)
    case_input = None
    if cases.extended(args):
        path = project.config_path(args)
        data = cases.encoded_config(args)
        if path.exists() and path.read_bytes() != data:
            raise ValueError("existing case inputs differ; choose a fresh --run")
        inputs.write_if_changed(path, data)
        case_input = {"path": str(path), "sha256": hashlib.sha256(data).hexdigest(), "config": cases.config(args)}
    if "TMPDIR" in environment:
        Path(environment["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    before = project.resources(strict=timed)
    prepared = project.prepared_manifest(args)
    started = time.monotonic()
    with log.with_suffix(".launcher.log").open("w") as output:
        with subprocess.Popen(argv, env=environment, cwd=project.ROOT, stdout=output, stderr=subprocess.STDOUT) as child:
            try:
                try:
                    status = child.wait(timeout=args.timeout)
                finally:
                    if child.poll() is None:
                        child.terminate()
                        try: child.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            child.kill()
                            child.wait()
            except BaseException:
                if engine == "unity":
                    recovery.failed(project.BASE.scratch_path(args.project), result, child.poll())
                raise
    if status or not result.exists():
        if engine == "unity": recovery.failed(project.BASE.scratch_path(args.project), result, status)
        raise RuntimeError(f"fixture failed (exit {status}); inspect {log}; no license changes attempted")
    data = json.loads(result.read_text())
    if not data["full_ids_tags_updates_checked"] or data["max_kernel_runners"] != args.runners:
        raise RuntimeError("unexpected runner budget or missing full identity checks")
    if engine == "unity":
        metadata = json.loads(Path(str(result) + ".environment.json").read_text())
        if (not metadata.get("finally_restored") or not metadata.get("measurement_completed")
                or not metadata.get("burst_enabled") or metadata.get("burst_safety_checks") is not False
                or metadata.get("burst_force_safety_checks") is not False or metadata.get("burst_debug") is not False):
            raise RuntimeError("controlled Burst flags or preference restoration not proven")
        checkpoint = recovery.successful(project.BASE.scratch_path(args.project), result, metadata)
    after = project.resources(strict=timed)
    if case_input and hashlib.sha256(Path(case_input["path"]).read_bytes()).hexdigest() != case_input["sha256"]:
        raise RuntimeError("case input changed during execution; retain raw output and rerun")
    if timed and before["power"].splitlines()[0] != after["power"].splitlines()[0]:
        raise RuntimeError("power source changed during the engine process; retain raw output and rerun")
    try:
        checked = project.prepared_manifest(args)
        if checked["marker_sha256"] != prepared["marker_sha256"]:
            raise ValueError("prepared marker changed during execution; run prepare")
    except ValueError as error:
        actual = inputs.prepared(project.BASE.scratch_path(args.project))
        bootstrap = engine == "unity" and inputs.bootstrap_changes(prepared["input_manifest"], actual)
        project.BASE.write_json(result.with_suffix(".unaccepted.json"), {
            "timings_accepted": False, "initial_import_bootstrap_outputs": bootstrap,
            "error": str(error), "prepared_project": prepared, "observed_prepared_inputs": actual})
        reason = "Unity initialized first-import metadata/settings" if bootstrap else "prepared inputs/templates changed"
        raise RuntimeError(reason + "; measurements are unaccepted; run prepare and a fresh actual smoke") from error
    project.BASE.write_json(result.with_suffix(".launch.json"), {
        "command": argv, "nice": 10, "before": before, "wall_seconds": time.monotonic() - started,
        "execution_lane": lane, "timings_accepted": timed, "strict_timing_guard_enforced": timed,
        "executable": executable,
        "case_input": case_input,
        "initial_preference_checkpoint": checkpoint if engine == "unity" else None,
        "prepared_project": prepared, "prepared_inputs_unchanged_after_run": True,
        "environment_sha256": (hashlib.sha256(Path(str(result) + ".environment.json").read_bytes()).hexdigest()
                               if engine == "unity" else None),
        "after": after, "resource_guards_passed": not before.get("busy_tools") and not after.get("busy_tools"),
        "result_sha256": hashlib.sha256(result.read_bytes()).hexdigest()})
    summary = {"result": str(result), "execution_lane": lane, "timings_accepted": timed,
               "state_audit_passed": True}
    if timed:
        summary.update(p50_ms=data["p50_ms"], p95_ms=data["p95_ms"])
    print(json.dumps(summary))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "build-command", "build-bevy", "command", "run-bevy", "run-unity", "recover-unity", "verify"])
    parser.add_argument("--case", choices=cases.NAMES, default="query_filter")
    parser.add_argument("--project", type=Path)
    parser.add_argument("--rust-binary", type=Path)
    parser.add_argument("--engine", choices=["bevy", "unity"], default="unity")
    parser.add_argument("--count", type=int, default=100_000)
    parser.add_argument("--runners", type=int, choices=[1, 15], default=1)
    parser.add_argument("--archetypes", type=int, choices=[8, 64], default=8)
    parser.add_argument("--percent", type=int, choices=[0, 1, 10, 100], default=10)
    parser.add_argument("--distribution", choices=["spread", "clustered"], default="spread")
    parser.add_argument("--topology", choices=["chain", "fork-join"], default="chain")
    parser.add_argument("--length", type=int, choices=[0, 4, 16, 64], default=4)
    parser.add_argument("--trim", action="store_true", help="explicit trim lane for resize; reuse is the default")
    parser.add_argument("--batch", type=int, default=4096)
    parser.add_argument("--warmup", type=int, default=120)
    parser.add_argument("--frames", type=int, default=300)
    parser.add_argument("--run", default="query-smoke-01")
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--reference", type=Path)
    parser.add_argument("--checkpoint", type=Path, help="known durable checkpoint for explicit preference-only batch recovery")
    parser.add_argument("--smoke", action="store_true", help="correctness only, <=10k entities/10steps; never accept timings")
    args = parser.parse_args()
    if args.project is None: args.project = project.EXTENDED_PROJECT if cases.extended(args) else project.PROJECT
    if args.rust_binary is None: args.rust_binary = project.rust_target(args) / "release" / project.binary_name(args)
    if args.count < 64 or args.count > 2**31 - 1 or args.warmup < 0 or args.frames < 1 or args.batch < 1 or args.timeout < 1:
        parser.error("invalid fixture size/step parameters")
    if args.warmup + args.frames > 2**31 - 1:
        parser.error("combined step count exceeds the current Unity loop limit")
    if args.action in ["command", "run-bevy", "run-unity", "verify"]:
        try: project.execution_lane(args)
        except ValueError as error: parser.error(str(error))
    if not args.run or args.run in [".", ".."] or any(part in args.run for part in ["/", "\\"]):
        parser.error("--run must be one folder name")
    if args.action == "prepare": project.prepare(args)
    elif args.action == "build-command": print(json.dumps(project.build_command(args), indent=2))
    elif args.action == "build-bevy": build.build(args)
    elif args.action == "command":
        command, environment, _, _, _ = project.command(args, args.engine)
        print(json.dumps({"command": command, "environment": {key: environment[key] for key in
                         ["UPM_CACHE_ROOT", "UPM_GIT_LFS_CACHE_PATH", "TMPDIR"] if key in environment}}, indent=2))
    elif args.action == "verify": verify.verify(args)
    elif args.action == "recover-unity":
        import recover
        recover.run(args)
    else: run(args, "bevy" if args.action == "run-bevy" else "unity")


if __name__ == "__main__": main()
