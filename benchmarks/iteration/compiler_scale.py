"""Optional Roslyn scaling probe with generated components and independent modules."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import queue
import re
import shutil
import statistics
import subprocess
import sys
import time
import threading


def source(module, file, revision, rate=None):
    rate = f"{revision + 1}.0f" if rate is None else rate
    return f"""using Ara;
namespace Module{module};
/// <summary>State for module {module}, file {file}.</summary>
[Component] public partial struct State{file} {{
    /// <summary>Current amount.</summary>
    public float Amount;
    /// <summary>Current velocity.</summary>
    public System.Numerics.Vector3 Velocity;
}}
/// <summary>Updates one generated component column.</summary>
public static partial class Update{file} {{
    [Query] static void Run(ref State{file} state, in Tick tick) {{
        state.Amount += tick.Dt * {rate};
    }}
}}
"""


def loaded_runtime(pid):
    """Observe the loaded CoreCLR image instead of inferring a runtime from the SDK."""
    try:
        if sys.platform == "darwin":
            listing = subprocess.run(["/usr/sbin/lsof", "-a", "-p", str(pid), "-Fn"],
                                     capture_output=True, text=True, timeout=10, check=True).stdout
        elif sys.platform.startswith("linux"):
            listing = Path(f"/proc/{pid}/maps").read_text(encoding="utf-8")
        else:
            return dict(version=None, image=None, error="runtime image observation is implemented for macOS/Linux only")
        match = re.search(r"(/[^\r\n]*?/shared/Microsoft\.NETCore\.App/([^/\r\n]+)/libcoreclr\.(?:dylib|so))", listing)
        if match:
            return dict(version=match[2], image=match[1], error=None)
        return dict(version=None, image=None, error="CoreCLR image was not present in the process mapping listing")
    except (OSError, subprocess.SubprocessError) as error:
        return dict(version=None, image=None, error=str(error))


def finish_server(process):
    """Reap this exact child and obtain its lifetime peak RSS where wait4 is available."""
    if not hasattr(os, "wait4"):
        try:
            process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise
        return None
    deadline = time.monotonic() + 30
    while True:
        pid, status, usage = os.wait4(process.pid, os.WNOHANG)
        if pid:
            process.returncode = os.waitstatus_to_exitcode(status)
            # macOS reports bytes; Linux and the other supported Unix hosts use KiB.
            return usage.ru_maxrss if sys.platform == "darwin" else usage.ru_maxrss * 1024
        if time.monotonic() >= deadline:
            process.kill()
            _, status, _ = os.wait4(process.pid, 0)
            process.returncode = os.waitstatus_to_exitcode(status)
            raise subprocess.TimeoutExpired(process.args, 30)
        time.sleep(0.01)


def run_case(repo, server, modules, files, sample_count, dotnet_host, label, edit_all, dependency_fanout=False, *, warmups=3, project_root=None, request_timeout=120):
    server_hash = hashlib.sha256(server.read_bytes()).hexdigest()
    project = project_root or repo / "target/iteration-perf" / f"compiler-scale-{os.getpid()}-{label}"
    if project.exists():
        raise FileExistsError(f"fixture must be fresh: {project}")
    for module in range(modules):
        directory = project / "content" / f"module{module}"
        directory.mkdir(parents=True, exist_ok=True)
        manifest = dict(name=f"module{module}")
        if dependency_fanout:
            manifest["references"] = ["core"]
        (directory / f"module{module}.module.json").write_text(json.dumps(manifest), encoding="utf-8")
        for file in range(files):
            (directory / f"State{file}.cs").write_text(source(module, file, 0, "Shared.Rule.Rate" if dependency_fanout else None), encoding="utf-8")
    if dependency_fanout:
        directory = project / "content/core"
        directory.mkdir()
        (directory / "core.module.json").write_text('{"name":"core"}', encoding="utf-8")
    artifacts = []
    samples = []
    with subprocess.Popen([dotnet_host, str(server), "--project-session", "--parent", str(os.getpid())],
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
        responses = queue.Queue()
        stderr = []
        stderr_bytes = 0

        def read_stdout():
            try:
                for line in process.stdout:
                    responses.put(line)
            finally:
                responses.put(None)

        def read_stderr():
            nonlocal stderr_bytes
            for line in process.stderr:
                stderr_bytes += len(line.encode("utf-8"))
                if stderr_bytes <= 4 * 1024 * 1024:
                    stderr.append(line)

        readers = [threading.Thread(target=read_stdout, daemon=True), threading.Thread(target=read_stderr, daemon=True)]
        for reader in readers:
            reader.start()
        try:
            for revision in range(sample_count + warmups):
                if dependency_fanout:
                    (project / "content/core/Rule.cs").write_text(
                        f"namespace Shared; public static class Rule {{ public const float Rate = {revision + 1}.0f; }}", encoding="utf-8")
                else:
                    for module in range(modules):
                        for file in range(files if edit_all else 1):
                            (project / "content" / f"module{module}" / f"State{file}.cs").write_text(source(module, file, revision), encoding="utf-8")
                start = time.perf_counter()
                process.stdin.write(json.dumps(dict(project=str(project), authoringRoot="content", domain="gameplay", operation="--compile-project")) + "\n")
                process.stdin.flush()
                try:
                    response = responses.get(timeout=request_timeout)
                except queue.Empty as error:
                    raise TimeoutError(f"compiler request {revision} exceeded {request_timeout}s") from error
                if not response:
                    raise RuntimeError("compiler exited before response: " + "".join(stderr))
                if stderr_bytes > 4 * 1024 * 1024:
                    raise RuntimeError("compiler stderr exceeded 4 MiB")
                result = json.loads(response)
                elapsed = (time.perf_counter() - start) * 1000
                errors = [d for d in result["diagnostics"] if d.get("severity") == "error"]
                expected = {f"module{i}" for i in range(modules)} | ({"core"} if dependency_fanout else set())
                actual = [m["name"] for m in result["modules"]]
                if len(actual) != len(expected) or set(actual) != expected:
                    raise RuntimeError(f"incorrect module inventory: {actual}")
                for module in result["modules"]:
                    expected_types = 0 if module["name"] == "core" else files
                    schema = module.get("schema") or {}
                    if schema.get("$schema") != "ara://schema/types/1" or len(schema.get("types", {})) != expected_types:
                        raise RuntimeError(f"incorrect reflected schema: {module['name']}")
                if errors or any(m["result"] != "compiled" for m in result["modules"]):
                    raise RuntimeError(errors or "expected a full compile of every module")
                samples.append(elapsed)
                artifacts.append({m["name"]: dict(
                    schema=hashlib.sha256(json.dumps(m["schema"], sort_keys=True, separators=(",", ":")).encode()).hexdigest(),
                    pe=hashlib.sha256((project / m["assemblyPath"]).read_bytes()).hexdigest(),
                    pdb=hashlib.sha256((project / m["assemblyPath"]).with_suffix(".pdb").read_bytes()).hexdigest())
                    for m in result["modules"]})
            runtime = loaded_runtime(process.pid)
        except BaseException:
            process.kill()
            raise
        finally:
            process.stdin.close()
            peak_rss = finish_server(process)
            for reader in readers:
                reader.join(timeout=2)
            if any(reader.is_alive() for reader in readers):
                raise RuntimeError("compiler output did not close after exit")
        if process.returncode or stderr_bytes > 4 * 1024 * 1024:
            raise RuntimeError("compiler exit/output failure: " + "".join(stderr))
        trailing = []
        while not responses.empty():
            if (line := responses.get_nowait()) is not None:
                trailing.append(line)
        if trailing:
            raise RuntimeError("unexpected trailing compiler output")
    warm = samples[warmups:]
    report = dict(modules=modules + int(dependency_fanout), files_per_module=files, generated_components=modules * files,
                  edited_sources_per_request=1 if dependency_fanout else modules * (files if edit_all else 1),
                  dependency_fanout=dependency_fanout, artifact_sha256_per_request=artifacts,
                  first_ms=samples[0], all_ms=samples, warmup_count=warmups, warm_ms=warm, p50_ms=statistics.median(warm),
                  p95_ms=sorted(warm)[math.ceil(0.95 * len(warm)) - 1],
                  server=str(server), server_sha256=server_hash, runtime=runtime,
                  dotnet_host=str(Path(dotnet_host).resolve()), platform=platform.platform(),
                  architecture=platform.machine(), peak_rss_bytes=peak_rss)
    if hashlib.sha256(server.read_bytes()).hexdigest() != server_hash:
        raise RuntimeError("compile-server bytes changed during the benchmark; rerun against a stable server")
    return report


def main():
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=repo / "dotnet/artifacts/bin/Ara.CompileServer/release/Ara.CompileServer.dll")
    parser.add_argument("--reference-server", type=Path, help="also measure this baseline in a fresh process")
    parser.add_argument("--reverse", action="store_true", help="run optimized before reference (requires --reference-server)")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--modules", type=int, default=12)
    parser.add_argument("--files", type=int, default=12)
    parser.add_argument("--samples", type=int, default=8)
    parser.add_argument("--warmups", type=int, default=3)
    parser.add_argument("--project-root", type=Path, help="fresh absolute fixture path; use the same path between sequential processes")
    parser.add_argument("--request-timeout", type=float, default=120)
    parser.add_argument("--dotnet-host", type=Path, help="pin the dotnet executable instead of PATH")
    parser.add_argument("--edit-all", action="store_true", help="change every source before each request instead of one per module")
    parser.add_argument("--dependency-fanout", action="store_true", help="edit one shared constant; rebuild all unchanged consumers against its new image")
    args = parser.parse_args()
    if min(args.modules, args.files, args.samples) < 1:
        parser.error("modules, files and samples must be positive")
    if args.warmups < 1 or args.request_timeout <= 0:
        parser.error("warmups and request timeout must be positive")
    if args.project_root is not None and (not args.project_root.is_absolute() or args.reference_server is not None):
        parser.error("project-root must be absolute and used with one server per invocation")
    if args.reverse and args.reference_server is None:
        parser.error("--reverse requires --reference-server")
    if args.edit_all and args.dependency_fanout:
        parser.error("--edit-all and --dependency-fanout describe different edit workloads")
    cases = [("optimized", args.server.resolve())]
    if args.reference_server is not None:
        cases.insert(0, ("reference", args.reference_server.resolve()))
    if args.reverse:
        cases.reverse()
    if any(not path.is_file() for _, path in cases):
        parser.error("compile server missing; build/test the compiler project first")
    dotnet_host = str(args.dotnet_host.resolve()) if args.dotnet_host else shutil.which("dotnet")
    if dotnet_host is None:
        parser.error("dotnet host is not on PATH")
    results = {label: run_case(repo, path, args.modules, args.files, args.samples, dotnet_host, label, args.edit_all, args.dependency_fanout,
                                     warmups=args.warmups, project_root=args.project_root, request_timeout=args.request_timeout)
               for label, path in cases}
    report = results["optimized"] if args.reference_server is None else dict(run_order=[label for label, _ in cases], **results)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
