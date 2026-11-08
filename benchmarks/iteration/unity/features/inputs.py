"""Content manifests distinguish repository templates from prepared Unity inputs."""
import hashlib
import json


def digest(data):
    return hashlib.sha256(data).hexdigest()


def manifest_digest(manifest):
    return digest(json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode())


def templates(here, wrapper, helper):
    root = here.parents[3]
    files = [path for path in (here / "unity/Assets").rglob("*") if path.is_file()]
    files += [path for path in (here / "extended/unity/Assets").rglob("*") if path.is_file()]
    files += [path for path in (here / "editor").rglob("*") if path.is_file()]
    files += list(here.glob("*.py")) + list((here / "rust/src").rglob("*.rs"))
    files += [here / "rust/Cargo.toml", wrapper, helper]
    if (here / "rust/Cargo.lock").is_file(): files.append(here / "rust/Cargo.lock")
    return {str(path.relative_to(root)): digest(path.read_bytes()) for path in sorted(files)}


def prepared(root):
    files = []
    for name in ["Assets", "Packages", "ProjectSettings"]:
        folder = root / name
        if folder.is_symlink():
            raise ValueError("prepared Unity inputs must not contain symlinks; run prepare")
        for path in folder.rglob("*"):
            if path.is_symlink():
                raise ValueError("prepared Unity inputs must not contain symlinks; run prepare")
            if path.is_file():
                files.append(path)
    return {str(path.relative_to(root)): digest(path.read_bytes()) for path in sorted(files)}


def differences(expected, actual):
    return {"added": sorted(actual.keys() - expected.keys()),
            "removed": sorted(expected.keys() - actual.keys()),
            "changed": sorted(key for key in expected.keys() & actual.keys() if expected[key] != actual[key])}


def check(root, marker_name, template_inputs, editor, packages):
    marker_path = root / marker_name
    raw = marker_path.read_bytes()
    marker = json.loads(raw)
    if marker.get("input_manifest_version") != 1:
        raise ValueError("prepared marker lacks complete input tracking; run prepare")
    if marker["editor"] != editor or marker["requested_packages"] != packages:
        raise ValueError("current Editor/package templates differ from preparation; run prepare")
    if template_inputs != marker["template_inputs"]:
        raise ValueError("repository templates changed since preparation; run prepare: " +
                         json.dumps(differences(marker["template_inputs"], template_inputs)))
    actual = prepared(root)
    if actual != marker["prepared_inputs"]:
        raise ValueError("prepared Unity inputs changed since preparation; run prepare: " +
                         json.dumps(differences(marker["prepared_inputs"], actual)))
    return {"marker_path": str(marker_path), "marker_sha256": digest(raw),
            "input_manifest_sha256": manifest_digest(marker), "input_manifest": marker}


def bootstrap_changes(manifest, actual):
    """Recognize first-import file creation, but never accept it as a timed run."""
    before = manifest["prepared_inputs"]
    already_imported = "Packages/packages-lock.json" in before and "ProjectSettings/ProjectSettings.asset" in before
    changed = differences(before, actual)
    generated = changed["added"] + changed["changed"]
    return not already_imported and not changed["removed"] and bool(generated) and all(
        path.startswith("ProjectSettings/") or path == "Packages/packages-lock.json" or
        (path.startswith("Assets/") and path.endswith(".meta")) for path in generated)


def sync_assets(source, destination, generated_paths):
    """Replace owned copies, pruning stale sources while keeping valid importer GUIDs."""
    expected = {str(path.relative_to(source)) for path in source.rglob("*") if path.is_file()}
    expected.update(generated_paths)
    allowed = set(expected)
    for name in expected:
        allowed.add(name + ".meta")
        parent = destination.joinpath(name).parent
        while parent != destination:
            allowed.add(str(parent.relative_to(destination)) + ".meta")
            parent = parent.parent
    prepared(destination.parent)  # Reject symlinks before touching owned copies.
    for path in list(destination.rglob("*")):
        if path.is_file() and str(path.relative_to(destination)) not in allowed:
            path.unlink()
    for path in sorted(destination.rglob("*"), key=lambda item: len(item.parts), reverse=True):
        if path.is_dir() and not any(path.iterdir()):
            path.rmdir()
    for path in source.rglob("*"):
        if path.is_file():
            write_if_changed(destination / path.relative_to(source), path.read_bytes())


def write_if_changed(path, data):
    if path.exists() and path.read_bytes() == data:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
