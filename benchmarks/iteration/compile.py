"""Optional CPU-only Roslyn project benchmark; Python is not a build prerequisite."""
import argparse
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import time


def main():
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=repo / "dotnet/artifacts/bin/Ara.CompileServer/release/Ara.CompileServer.dll")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    server = args.server.resolve()
    if not server.is_file():
        parser.error("compile server missing; run dotnet test dotnet/tests/Ara.CompileServer.Tests -c Release first")
    project = repo / "target/iteration-perf" / ("csharp-project-" + str(os.getpid()))
    sources = project / "content"
    sources.mkdir(parents=True, exist_ok=True)
    for i in range(128):
        methods = "\n".join(f"    public static float M{j}(float x) {{ return x * {j+1}.0f + {i}.0f; }}" for j in range(48))
        (sources / f"Code{i}.cs").write_text(f"public static class Code{i} {{\n{methods}\n}}\n", encoding="utf-8")
    values = []
    # No Rust publication stamps are created: every request deliberately compiles
    # the whole project. This measures Roslyn work, not a cache hit or MSBuild.
    with subprocess.Popen(
        ["dotnet", str(server), "--project-session", "--parent", str(os.getpid())],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    ) as process:
        try:
            for i in range(17):
                (sources / "Edit.cs").write_text(f"public static class Edit {{ public static int Value => {i}; }}", encoding="utf-8")
                start = time.perf_counter()
                process.stdin.write(json.dumps(dict(project=str(project), authoringRoot="content", domain="gameplay", operation="--compile-project")) + "\n")
                process.stdin.flush()
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError(process.stderr.read())
                result = json.loads(line)
                elapsed = (time.perf_counter() - start) * 1000
                errors = [d for d in result["diagnostics"] if d.get("severity") == "error"]
                if errors or result["modules"][0]["result"] != "compiled":
                    raise RuntimeError(errors or "expected full compilation")
                values.append(elapsed)
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        if process.returncode:
            raise RuntimeError(process.stderr.read())
    warm = values[5:]
    report = dict(first_ms=values[0], warm_ms=warm, p50_ms=statistics.median(warm),
                  p95_ms=sorted(warm)[math.ceil(0.95 * len(warm)) - 1], files=129, methods=6144,
                  server=str(server))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
