"""Feature-only checkpoints bind known initial state; unresolved failures block Unity."""
import hashlib
import fcntl
import json
import os
import re
import tempfile
from pathlib import Path

KEYS = {"BurstCompilation", "BurstCompileSynchronously", "BurstShowTimings", "BurstDebug", "BurstForceSafetyChecks"}
OPTIONS = ("initial_burst_compilation", "initial_burst_safety_checks", "initial_burst_force_safety_checks",
           "initial_burst_debug", "initial_burst_compile_synchronously")
PENDING = ".ara-preference-recovery-required.json"
ROOT = Path(__file__).resolve().parents[4]
REGISTRY = ROOT / "target/perf-scale/feature-preference-recovery/projects.json"


def checked_project(project):
    project = Path(project).resolve()
    if not project.is_relative_to((ROOT / "target").resolve()): raise ValueError("preference project is outside owned target")
    markers = [(".ara-query-feature.json", ("query_filter-v1", "extended-v1")),
               (".ara-s8-current.json", ("s8-current-v1",))]
    present = [(path, families) for name, families in markers if (path := project / name).is_file()]
    if len(present) != 1: raise ValueError("preference project needs exactly one owned fixture marker")
    path, families = present[0]
    marker = json.loads(path.read_text())
    if marker.get("repository_root") != str(ROOT) or marker.get("fixture") not in families:
        raise ValueError("preference project marker ownership differs")
    return project


def projects():
    if not REGISTRY.exists(): return []
    data = json.loads(REGISTRY.read_text())
    if data.get("version") != 1 or data.get("repository_root") != str(ROOT): raise ValueError("invalid shared preference project registry")
    return [checked_project(path) for path in data["projects"]]


def register(project):
    """Preparation registers every controlled project before any Unity invocation."""
    REGISTRY.parent.mkdir(parents=True, exist_ok=True)
    with REGISTRY.with_suffix(".lock").open("a") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        registered = sorted({str(path) for path in [*projects(), checked_project(project)]})
        with tempfile.NamedTemporaryFile(mode="w", dir=REGISTRY.parent, prefix="projects-", delete=False) as output:
            temporary = Path(output.name)
            json.dump({"version": 1, "repository_root": str(ROOT), "projects": registered}, output, indent=2)
            output.flush(); os.fsync(output.fileno())
        try: temporary.replace(REGISTRY)
        finally:
            if temporary.exists(): temporary.unlink()


def adapt_wrapper(source):
    """Guard every finally setter if durable capture failed before mutation."""
    start = "                    try { options.EnableBurstCompilation = initialCompilation; }"
    end = "                            finally { options.EnableBurstDebug = initialDebug; }\n                        }\n                    }"
    at = source.find(start)
    stop = source.find(end, at)
    if at < 0 or stop < 0: raise ValueError("feature restoration seam changed; review wrapper")
    block = source[at:stop + len(end)]
    replacements = {
        "            int exitCode = 1;": "            int exitCode = 1;\n            bool featureMutationStarted = false;",
        "                options.EnableBurstCompilation = true;":
            "                AraFeatureControl.PreferenceRecovery.Save(environmentPath, environment);\n"
            "                featureMutationStarted = true;\n"
            '                Debug.Log("ARA_FEATURE_MUTATION_STARTED");\n'
            "                options.EnableBurstCompilation = true;",
        block: "                    if (featureMutationStarted)\n                    {\n" + block + "\n                    }",
        "                        RestorePreferences(initialPreferences);":
            "                        if (featureMutationStarted) RestorePreferences(initialPreferences);",
        "                            File.WriteAllText(environmentPath, JsonUtility.ToJson(environment, true));":
            "                            File.WriteAllText(environmentPath, JsonUtility.ToJson(environment, true));\n"
            "                            if (featureMutationStarted) AraFeatureControl.PreferenceRecovery.Complete(environmentPath, environment);",
        "                S8Unity.Benchmark.Run();":
            '                Debug.Log("ARA_FEATURE_ENTERING_BENCHMARK");\n                S8Unity.Benchmark.Run();',
    }
    for before, after in replacements.items():
        if source.count(before) != 1: raise ValueError("feature checkpoint seam changed; review wrapper")
        source = source.replace(before, after)
    return source


def checkpoint_path(result):
    return Path(str(result) + ".environment.json.initial.json")


def owned(project, path):
    project, path = Path(project).resolve(), Path(path)
    if path.is_symlink() or not path.resolve().is_relative_to(project / "results"):
        raise ValueError("preference checkpoint must remain under owned scratch results")
    for parent in path.parents:
        if parent == project: break
        if parent.is_symlink(): raise ValueError("preference checkpoint path contains a symlink")
    return path.resolve()


def checked_checkpoint(project, path, result=None):
    path = owned(project, path)
    raw = path.read_bytes(); data = json.loads(raw)
    if data.get("version") != 1 or not isinstance(data.get("invocation_id"), str) or len(data["invocation_id"]) != 32:
        raise ValueError("invalid checkpoint version/invocation")
    if Path(data["project"]).resolve() != Path(project).resolve(): raise ValueError("checkpoint project differs")
    bound_result = owned(project, Path(data["result_path"]))
    if path != checkpoint_path(bound_result) or (result is not None and bound_result != Path(result).resolve()):
        raise ValueError("checkpoint belongs to another invocation/result")
    initial_raw = data["initial_json"].encode()
    if hashlib.sha256(initial_raw).hexdigest() != data["initial_state_sha256"]:
        raise ValueError("checkpoint initial-state bytes differ")
    initial = json.loads(initial_raw)
    if initial.get("measurement_completed") is not False: raise ValueError("checkpoint must precede measurement")
    rows = initial.get("initial_editor_preferences", [])
    if len(rows) != 5 or {row.get("key") for row in rows} != KEYS or any(
            type(row.get("existed")) is not bool or type(row.get("value")) is not bool for row in rows):
        raise ValueError("checkpoint must capture exactly five known preference keys/presence/values")
    if any(type(initial.get(key)) is not bool for key in OPTIONS): raise ValueError("checkpoint initial options are incomplete")
    return data, initial, hashlib.sha256(raw).hexdigest()


def checked_receipt(project, path, receipt_path, recovery_attempt=None):
    data, initial, digest = checked_checkpoint(project, path)
    receipt_path = owned(project, receipt_path); raw = receipt_path.read_bytes(); receipt = json.loads(raw)
    for key, value in [("version", 1), ("invocation_id", data["invocation_id"]), ("checkpoint_sha256", digest),
                       ("initial_state_sha256", data["initial_state_sha256"]), ("project", data["project"]), ("result_path", data["result_path"])]:
        if receipt.get(key) != value: raise ValueError("restoration receipt is not bound to checkpoint/invocation")
    if receipt.get("kind") not in ("normal_finally", "explicit_recovery") or any(receipt.get(key) is not True for key in
            ["restored", "initial_options_restored", "known_preferences_restored"]):
        raise ValueError("checkpoint restoration is not complete")
    if receipt["kind"] == "explicit_recovery" and not re.fullmatch(r"[0-9a-fA-F]{32}", receipt.get("recovery_attempt_id") or ""):
        raise ValueError("invalid recovery receipt attempt ID")
    suffix = ".restoration.json" if receipt["kind"] == "normal_finally" else ".recovery-" + receipt["recovery_attempt_id"] + ".json"
    if receipt_path != Path(str(Path(path).resolve()) + suffix): raise ValueError("restoration receipt path/kind differs")
    if recovery_attempt is not None and (receipt["kind"] != "explicit_recovery" or receipt.get("recovery_attempt_id") != recovery_attempt):
        raise ValueError("restoration receipt belongs to another recovery attempt")
    return {"path": str(path), "sha256": digest, "invocation_id": data["invocation_id"],
            "initial_state_sha256": data["initial_state_sha256"], "receipt": str(receipt_path),
            "receipt_sha256": hashlib.sha256(raw).hexdigest(), "restored": True}


def begin_recovery(project, path, attempt):
    """A new recovery mutation needs new proof, even if a normal receipt exists."""
    project = checked_project(project)
    if not re.fullmatch(r"[0-9a-fA-F]{32}", attempt): raise ValueError("invalid recovery attempt ID")
    data, _, digest = checked_checkpoint(project, path)
    pending = project / PENDING
    previous = json.loads(pending.read_text()) if pending.exists() else None
    bound = {"path": str(Path(path).resolve()), "sha256": digest}
    if previous is not None and previous.get("checkpoint") != bound:
        raise ValueError("recovery attempt does not match pending invocation")
    record = {"stage": "explicit_recovery_started", "checkpoint": bound,
              "result": data["result_path"], "recovery_attempt_id": attempt,
              "restoration_proven": False, "previous_failure": previous}
    with tempfile.NamedTemporaryFile(mode="w", dir=project, prefix="recovery-", delete=False) as output:
        temporary = Path(output.name)
        json.dump(record, output, indent=2)
        output.flush(); os.fsync(output.fileno())
    try: temporary.replace(pending)
    finally:
        if temporary.exists(): temporary.unlink()
    return record


def successful(project, result, environment):
    path = checkpoint_path(result); data, initial, _ = checked_checkpoint(project, path, result)
    if any(environment.get(key) != initial[key] for key in OPTIONS) or environment.get("initial_editor_preferences") != initial["initial_editor_preferences"]:
        raise ValueError("environment initial state differs from durable checkpoint")
    if environment.get("finally_restored") is not True: raise ValueError("finally restoration is unproven")
    return checked_receipt(project, path, Path(str(path) + ".restoration.json"))


def ensure_clear(project):
    for current in {*projects(), checked_project(project)}:
        path = current / PENDING
        if path.exists(): raise ValueError("Unity preference recovery is unresolved; review " + str(path) + " before any owned feature Unity launch")
        for checkpoint in (current / "results").rglob("*.environment.json.initial.json"):
            resolved = False
            receipts = [Path(str(checkpoint) + ".restoration.json"), *sorted(checkpoint.parent.glob(checkpoint.name + ".recovery-*.json"))]
            for receipt in receipts:
                try:
                    checked_receipt(current, checkpoint, receipt)
                    resolved = True
                    break
                except (ValueError, OSError, KeyError): pass
            if not resolved: raise ValueError("unresolved durable preference checkpoint; review " + str(checkpoint))


def failed(project, result, exit_code):
    """A native crash may skip finally; never silently retry an unresolved mutation."""
    project = Path(project); result = owned(project, result); checkpoint = checkpoint_path(result)
    record = {"exit_code": exit_code, "result": str(result), "checkpoint": None, "restoration_proven": False}
    record["stage"] = "unknown_no_checkpoint"
    environment = Path(str(result) + ".environment.json")
    if checkpoint.exists():
        record["checkpoint"] = {"path": str(checkpoint), "sha256": hashlib.sha256(checkpoint.read_bytes()).hexdigest()}
        record["stage"] = "after_checkpoint_mutation_possible"
        try:
            checked_checkpoint(project, checkpoint, result)
            if environment.exists():
                record["restoration"] = successful(project, result, json.loads(environment.read_text()))
                record["restoration_proven"] = True
        except (ValueError, OSError, KeyError) as error:
            record["checkpoint_validation_error"] = str(error)
    result.with_suffix(".failure.json").write_text(json.dumps(record, indent=2) + "\n")
    if not record["restoration_proven"]:
        (project / PENDING).write_text(json.dumps(record, indent=2) + "\n")
    return record
