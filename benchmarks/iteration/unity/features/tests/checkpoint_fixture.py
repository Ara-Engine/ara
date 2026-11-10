"""Synthetic receipt construction for launcher contracts; never accesses EditorPrefs."""
import hashlib
import json
from pathlib import Path

import recovery


def initial():
    return {**{key: False for key in recovery.OPTIONS}, "measurement_completed": False,
            "initial_editor_preferences": [{"key": key, "existed": index % 2 == 0, "value": index % 2 == 0 and index % 3 == 0}
                                           for index, key in enumerate(sorted(recovery.KEYS))]}


def checkpoint(project, result, environment=None):
    path = recovery.checkpoint_path(result)
    snapshot = {key: environment[key] for key in [*recovery.OPTIONS, "initial_editor_preferences"]} if environment is not None else initial()
    snapshot["measurement_completed"] = False
    text = json.dumps(snapshot, separators=(",", ":"))
    data = {"version": 1, "invocation_id": "a" * 32, "project": str(Path(project).resolve()),
            "result_path": str(Path(result).resolve()), "initial_json": text,
            "initial_state_sha256": hashlib.sha256(text.encode()).hexdigest()}
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data))
    return path


def receipt(project, path, kind="normal_finally", restored=True, attempt=None):
    data, _, digest = recovery.checked_checkpoint(project, path)
    suffix = ".restoration.json" if kind == "normal_finally" else ".recovery-" + attempt + ".json"
    destination = Path(str(path) + suffix)
    destination.write_text(json.dumps({"version": 1, "kind": kind, "invocation_id": data["invocation_id"],
        "project": data["project"], "result_path": data["result_path"], "checkpoint_sha256": digest,
        "initial_state_sha256": data["initial_state_sha256"], "restored": restored,
        "initial_options_restored": restored, "known_preferences_restored": restored, "recovery_attempt_id": attempt}))
    return destination
