"""Correctness only: compare a retained project compiler with fresh helpers after edits.

Generated fixtures are private to a new output directory. Every response, source
manifest and successful image is retained. No request latency is measured.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import struct
import subprocess
import threading

from compiler_scale import loaded_runtime, source


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def manifest(root):
    return {str(p.relative_to(root)): sha(p) for p in sorted(root.rglob("*")) if p.is_file()}


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


class Child:
    """One owned compiler; bounded protocol reads and deadline/EOF cleanup."""
    def __init__(self, host, server, log, fresh=False):
        self.log, self.fresh = log, fresh
        self.stderr = bytearray()
        self.errors = []
        self.lines = queue.Queue()
        self.process = subprocess.Popen(
            [str(host), str(server), "--compile-project" if fresh else "--project-session",
             "--parent", str(os.getpid())], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE)

        def stdout():
            try:
                while line := self.process.stdout.readline(32 * 1024 * 1024 + 1):
                    if len(line) > 32 * 1024 * 1024 or self.lines.qsize() >= 16:
                        raise RuntimeError("compiler stdout overflow")
                    self.lines.put(line)
            except BaseException as error:
                self.errors.append(repr(error))
            finally:
                self.lines.put(None)

        def stderr():
            try:
                while block := self.process.stderr.read(4096):
                    remaining = 4 * 1024 * 1024 - len(self.stderr)
                    self.stderr.extend(block[:remaining])
                    if len(block) > remaining:
                        raise RuntimeError("compiler stderr overflow")
            except BaseException as error:
                self.errors.append(repr(error))

        self.readers = [threading.Thread(target=stdout, daemon=True), threading.Thread(target=stderr, daemon=True)]
        for reader in self.readers:
            reader.start()

    def request(self, request):
        self.process.stdin.write(json.dumps(request).encode() + b"\n")
        self.process.stdin.flush()
        if self.fresh:
            self.process.stdin.close()
        try:
            line = self.lines.get(timeout=120)
        except queue.Empty as error:
            raise TimeoutError("compiler response exceeded 120 seconds") from error
        if line is None or self.errors:
            raise RuntimeError("compiler protocol failed: " + repr(self.errors))
        return json.loads(line)

    def close(self, failed=False):
        cleanup = []
        try:
            if not self.process.stdin.closed:
                self.process.stdin.close()
            if failed and self.process.poll() is None:
                self.process.kill()
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                cleanup.append("compiler shutdown exceeded 15 seconds")
                self.process.kill()
                self.process.wait(timeout=10)
            for reader in self.readers:
                reader.join(timeout=2)
            if any(reader.is_alive() for reader in self.readers):
                cleanup.append("compiler output EOF missing")
            if any(line is not None for line in list(self.lines.queue)):
                cleanup.append("trailing compiler stdout")
            if self.process.returncode and not failed:
                cleanup.append(f"compiler exit {self.process.returncode}")
        finally:
            self.log.write_bytes(self.stderr)
            save(self.log.with_suffix(".cleanup.json"), dict(exit=self.process.returncode, errors=self.errors + cleanup))
        if self.errors or cleanup:
            raise RuntimeError("compiler cleanup failure: " + repr(self.errors + cleanup))


def fixture_text(module, file, bias):
    # Larger than ModuleText's 2 KiB small-text boundary, with non-ASCII UTF-16.
    return (source(module, file, 0, "Shared.Rule.Rate")
            + f"\n/// <summary>Constant propagation witness.</summary>\npublic static class Witness{file} {{ public static float Read() => Shared.Rule.Rate + {bias}f; }}\n"
            + "// unchanged UTF-16 padding " + "é🦀漢字" * 500 + "\n")


def validate(result, expected, failed, code, attempt):
    modules = {m["name"]: m for m in result["modules"]}
    if len(modules) != len(result["modules"]) or set(modules) != set(expected):
        raise RuntimeError("incorrect module inventory")
    errors = [row for row in result["diagnostics"] if row.get("severity") == "error"]
    if len(errors) != len(failed) or {row["data"]["module"] for row in errors} != failed:
        raise RuntimeError("incorrect error contract: " + repr(errors))
    for row in errors:
        owner, expected_code, prefix = code
        if row["code"] != (expected_code if row["data"]["module"] == owner else "ARA0925"):
            raise RuntimeError("incorrect rejection diagnostic code")
        if row["data"]["module"] == owner and prefix and not row["message"].startswith(prefix):
            raise RuntimeError("incorrect compiler diagnostic identity")
    for name, ids in expected.items():
        module = modules[name]
        if module["result"] != ("failed" if name in failed else "compiled"):
            raise RuntimeError("incorrect module result: " + name)
        if name in failed:
            if module.get("schema") is not None or module.get("fingerprint") is not None:
                raise RuntimeError("failed module published schema/fingerprint")
        else:
            schema = module["schema"]
            if schema["$schema"] != "ara://schema/types/1" or set(schema["types"]) != {f"State{i}" for i in ids}:
                raise RuntimeError("incorrect reflected type inventory: " + name)
            for definition in schema["types"].values():
                if (definition["kind"] != "component" or definition["module"] != name
                    or {key: field["type"] for key, field in definition["fields"].items()} != {"amount": "f32", "velocity": "vec3"}):
                    raise RuntimeError("incorrect component field schema")
        rows = [row["data"]["compile"] for row in result["diagnostics"]
                if row.get("data", {}).get("module") == name and row["data"]["compile"]["phase"] != "diagnostic"]
        if [row["phase"] for row in rows] != ["bound", "started", "completed"]:
            raise RuntimeError("incorrect lifecycle: " + name)
        if [row["attempt"] for row in rows] != [0, attempt, attempt]:
            raise RuntimeError("incorrect attempt binding")
        if rows[-1]["result"] != ("errors" if name in failed else "success"):
            raise RuntimeError("incorrect completion result")


def capture(project, result, directory, previous, failed):
    directory.mkdir()
    images = {}
    for module in result["modules"]:
        name = module["name"]
        pe = project / module["assemblyPath"]
        paths = [pe, pe.with_suffix(".pdb")]
        hashes = {p.suffix: sha(p) for p in paths if p.exists()}
        if name in failed:
            if hashes != previous.get(name, {}):
                raise RuntimeError("rejection changed published PE/PDB: " + name)
        else:
            if set(hashes) != {".dll", ".pdb"}:
                raise RuntimeError("missing compiler image")
            for path in paths:
                shutil.copyfile(path, directory / (name + path.suffix))
        images[name] = hashes
    save(directory / "response.json", result)
    save(directory / "images.json", images)
    return images


def witnesses(host, verifier, server, project, result, expected, failed, value, output):
    modules = [dict(name=m["name"], path=str(project / m["assemblyPath"]),
                    namespace="Shared" if m["name"] == "core" else "Module" + m["name"][6:],
                    witnesses=[f"Witness{i}" for i in expected[m["name"]]])
               for m in result["modules"] if m["name"] not in failed]
    save(output, dict(sdk=str(server.parent), rateBits=struct.unpack("<i", struct.pack("<f", value))[0], modules=modules))
    with output.with_suffix(".stdout").open("w") as stdout, output.with_suffix(".stderr").open("w") as stderr:
        process = subprocess.Popen([str(host), str(verifier), str(output)], stdout=stdout, stderr=stderr)
        try:
            exit_code = process.wait(timeout=120)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
    if any(output.with_suffix(suffix).stat().st_size > 4 * 1024 * 1024 for suffix in [".stdout", ".stderr"]):
        raise RuntimeError("witness helper output overflow")
    if exit_code:
        raise RuntimeError("witness execution failed: " + output.with_suffix(".stderr").read_text())
    proof = json.loads(output.with_suffix(".stdout").read_text())
    if proof["witnesses"] != sum(len(expected[name]) for name in expected if name not in failed):
        raise RuntimeError("incorrect executed witness count")
    return proof


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--verifier", type=Path, required=True)
    parser.add_argument("--dotnet-host", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--modules", type=int, default=8)
    parser.add_argument("--files", type=int, default=256)
    args = parser.parse_args()
    if args.modules < 1 or args.files < 2:
        parser.error("at least one module and two files required")
    host, server, verifier = (p.resolve() for p in [args.dotnet_host, args.server, args.verifier])
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    project = out / "PROJECT"
    expected = {"core": []} | {f"module{i}": list(range(args.files)) for i in range(args.modules)}
    core = project / "content/core"
    core.mkdir(parents=True)
    (core / "core.module.json").write_text('{"name":"core"}')
    for module in range(args.modules):
        directory = project / "content" / f"module{module}"
        directory.mkdir()
        save(directory / f"module{module}.module.json", dict(name=f"module{module}", references=["core"]))
        for file in expected[f"module{module}"]:
            (directory / f"State{file}.cs").write_text(fixture_text(module, file, "0.0"), encoding="utf-8")
    framework = host.parent / "shared/Microsoft.NETCore.App"
    references = host.parent / "packs/Microsoft.NETCore.App.Ref"
    frozen = dict(sdk=manifest(server.parent), verifier=manifest(verifier.parent), host=sha(host),
                  framework=manifest(framework), references=manifest(references),
                  sources={str(p): sha(p) for p in [Path(__file__).resolve(), Path(__file__).with_name("compiler_scale.py").resolve()]})
    report = dict(quality_passed=False, timings_accepted=False, modules=args.modules, files=args.files,
                  frozen=frozen, cases=[], command=list(os.sys.argv))
    report["source_commit"] = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    report["dotnet_info"] = subprocess.run([str(host), "--info"], capture_output=True, text=True, timeout=30, check=True).stdout
    save(out / "inputs.json", frozen)
    warm = None
    previous = {}
    rate, bias = 1, 0.0
    try:
        warm = Child(host, server, out / "warm.stderr")
        for attempt, case in enumerate(["initial", "dependency", "bulk", "membership", "hot", "compiler-error", "compiler-repair", "policy-error", "policy-repair"], 1):
            failed, code = set(), None
            if case == "dependency":
                consumers_before = manifest(project / "content")
                rate = 2
            if case == "bulk":
                bias = 0.25
                for module in range(args.modules):
                    for file in expected[f"module{module}"]:
                        (project / "content" / f"module{module}" / f"State{file}.cs").write_text(fixture_text(module, file, "0.25"), encoding="utf-8")
            if case == "membership":
                for module in range(args.modules):
                    directory = project / "content" / f"module{module}"
                    (directory / "State0.cs").rename(directory / "Moved0.cs")
                    (directory / "State1.cs").unlink()
                    (directory / f"State{args.files}.cs").write_text(fixture_text(module, args.files, "0.25"), encoding="utf-8")
                    expected[f"module{module}"] = [0, *range(2, args.files + 1)]
            if case == "hot":
                for module in range(args.modules):
                    save(project / "content" / f"module{module}" / f"module{module}.module.json", dict(name=f"module{module}", references=["core"], hot=True))
            if case == "compiler-error":
                failed, code = set(expected), ("core", "ARA0900", "CS0103: ")
            if case == "compiler-repair":
                rate = 7
            if case == "policy-error":
                (project / "content/module0/Invalid.cs").write_text("public static class InvalidState { public static int Value; }")
                failed, code = {"module0"}, ("module0", "ARA0100", None)
            if case == "policy-repair":
                (project / "content/module0/Invalid.cs").unlink()
            (core / "Rule.cs").write_text(f"namespace Shared; public static class Rule {{ public const float Rate = {'Missing' if case == 'compiler-error' else str(rate) + '.0f'}; }}")
            sources = manifest(project / "content")
            if case == "dependency" and any(value != sources[path] for path, value in consumers_before.items() if path != "core/Rule.cs"):
                raise RuntimeError("dependency edit changed a consumer")
            directory = out / f"{attempt:02d}-{case}"
            directory.mkdir()
            save(directory / "sources.json", sources)
            request = dict(project=str(project), authoringRoot="content", domain="gameplay", operation="--compile-project",
                           diagnosticContext=dict(producer="quality", instance="c0ffee00000000000000000000000001", binding=1, attempt=attempt))
            save(directory / "request.json", request)
            row = dict(case=case, attempt=attempt, passed=False)
            report["cases"].append(row)
            reference = None
            for arm in ["warm", "fresh"]:
                child = warm if arm == "warm" else Child(host, server, directory / "fresh.stderr", fresh=True)
                success = False
                try:
                    result = child.request(request)
                    save(directory / (arm + "-raw.json"), result)
                    validate(result, expected, failed, code, attempt)
                    images = capture(project, result, directory / arm, previous, failed)
                    proof = witnesses(host, verifier, server, project, result, expected, failed, rate + bias, directory / (arm + "-witness.json"))
                    row[arm] = dict(images=images, witness=proof)
                    if reference is None:
                        reference = (result, images)
                    elif (result, images) != reference:
                        raise RuntimeError("exact warm/fresh response or PE/PDB parity failed")
                    success = True
                finally:
                    if arm == "fresh":
                        child.close(failed=not success)
            if manifest(project / "content") != sources:
                raise RuntimeError("compiler mutated fixture sources")
            previous = reference[1]
            row["passed"] = True
            print(json.dumps(dict(case=case, passed=True, witnesses_per_arm=row["warm"]["witness"]["witnesses"])), flush=True)
        report["runtime"] = loaded_runtime(warm.process.pid)
        if report["runtime"].get("image"):
            report["runtime"]["image_sha256"] = sha(Path(report["runtime"]["image"]))
        report["quality_passed"] = True
    except BaseException as error:
        report["failure"] = repr(error)
        raise
    finally:
        try:
            if warm is not None:
                warm.close(failed=not report["quality_passed"])
            report["frozen_unchanged"] = (manifest(server.parent) == frozen["sdk"] and manifest(verifier.parent) == frozen["verifier"]
                                           and sha(host) == frozen["host"] and manifest(framework) == frozen["framework"]
                                           and manifest(references) == frozen["references"] and all(sha(Path(p)) == h for p, h in frozen["sources"].items()))
            if not report["frozen_unchanged"]:
                raise RuntimeError("frozen inputs changed")
        except BaseException as error:
            report["quality_passed"] = False
            report["cleanup_failure"] = repr(error)
            raise
        finally:
            save(out / "result.json", report)


if __name__ == "__main__":
    main()
