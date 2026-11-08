"""Explicit owned-scratch preference recovery; never invokes a benchmark."""
import json
import os
from pathlib import Path
import subprocess
import uuid

import project
import recovery


def command(args):
    root = project.BASE.scratch_path(args.project)
    if args.checkpoint is None: raise ValueError("explicit --checkpoint required; no guessed baseline")
    path = recovery.owned(root, args.checkpoint)
    _, _, digest = recovery.checked_checkpoint(root, path)
    pending = root / recovery.PENDING
    if pending.exists():
        state = json.loads(pending.read_text())
        if not state.get("checkpoint") or state["checkpoint"] != {"path": str(path), "sha256": digest}:
            raise ValueError("checkpoint does not match pending invocation; no guessed baseline")
    project.prepared_manifest(args)
    attempt = uuid.uuid4().hex
    receipt = Path(str(path) + ".recovery-" + attempt + ".json")
    if receipt.exists(): raise ValueError("recovery receipt already exists; inspect its bound proof")
    log = Path(str(path) + ".recovery-" + attempt + ".log")
    if log.exists(): raise ValueError("recovery log exists; inspect prior attempt before retrying")
    environment = dict(os.environ, UPM_CACHE_ROOT=str(root / "upm-cache"),
        UPM_GIT_LFS_CACHE_PATH=str(root / "upm-cache/git-lfs"),
        TMPDIR=str(project.ROOT / ("target/perf-scale/ufx-t" if project.cases.extended(args) else "target/perf-scale/uf-t")))
    argv = ["/usr/bin/nice", "-n", "10", str(project.BASE.EDITOR), "-batchmode", "-nographics", "-projectPath", str(root),
        "-executeMethod", "AraFeatureControl.PreferenceRecovery.Recover", "-logFile", str(log),
        "-ara-feature-recover", str(path), digest, attempt]
    return argv, environment, path, receipt, log


def run(args):
    if project.BASE.active_editors(): raise ValueError("a Unity Editor is running; leave it untouched")
    argv, environment, checkpoint, receipt, log = command(args)
    Path(environment["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    with log.with_suffix(".launcher.log").open("x") as output:
        attempt = argv[-1]
        recovery.begin_recovery(args.project, checkpoint, attempt)
        with subprocess.Popen(argv, env=environment, cwd=project.ROOT, stdout=output, stderr=subprocess.STDOUT) as child:
            try: status = child.wait(timeout=args.timeout)
            finally:
                if child.poll() is None:
                    child.terminate()
                    try: child.wait(timeout=10)
                    except subprocess.TimeoutExpired: child.kill(); child.wait()
    if status: raise RuntimeError("recovery failed; retain checkpoint and pending guard, inspect " + str(log))
    proof = recovery.checked_receipt(project.BASE.scratch_path(args.project), checkpoint, receipt, attempt)
    pending = project.BASE.scratch_path(args.project) / recovery.PENDING
    if pending.exists():
        state = json.loads(pending.read_text())
        if not state.get("checkpoint") or state["checkpoint"]["sha256"] != proof["sha256"] or state.get("recovery_attempt_id") != attempt:
            raise ValueError("recovered checkpoint does not resolve the pending invocation")
        project.BASE.write_json(Path(str(checkpoint) + ".recovery-" + attempt + ".launch.json"), {"command": argv, "restoration": proof,
            "pending_failure": state, "timings_accepted": False, "benchmark_invoked": False})
        pending.unlink()
    print(json.dumps({"restoration": proof, "timings_accepted": False, "benchmark_invoked": False}))
