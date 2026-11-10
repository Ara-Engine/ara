"""Optional editor snapshot/query benchmark against a built compile server."""
import argparse
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import time


SOURCE = """
using Ara.Editor;
public sealed class SceneProbe : IEditorExtension
{
    public void Register(EditorRegistry registry)
    {
        registry.RegisterCommand("probe.snapshot", "Probe snapshot", context => context.Commands.Create(context.Scene.Entities.Count.ToString()));
        registry.RegisterCommand("probe.hierarchy", "Probe hierarchy", context => {
            long count = 0;
            foreach (var entity in context.Scene.Entities) {
                count += context.Scene.Children(entity.Id).Count;
                count += context.Scene.Siblings(entity.Id).Count;
            }
            context.Commands.Create(count.ToString());
        });
        registry.RegisterCommand("probe.names", "Probe names", context => {
            var count = 0;
            foreach (var entity in context.Scene.Entities.TakeLast(100)) {
                if (context.Scene.Find(entity.Name)?.Id != entity.Id) throw new InvalidOperationException("name mismatch");
                count++;
            }
            context.Commands.Create(count.ToString());
        });
        registry.RegisterCommand("probe.paths", "Probe paths", context => {
            var count = 0;
            foreach (var entity in context.Scene.Entities.TakeLast(100)) {
                var path = entity.Parent is null ? entity.Name : "Root/" + entity.Name;
                if (context.Scene.Find(path)?.Id != entity.Id) throw new InvalidOperationException("path mismatch");
                count++;
            }
            context.Commands.Create(count.ToString());
        });
    }
}
"""


def main():
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=repo / "dotnet/artifacts/bin/Ara.CompileServer/release/Ara.CompileServer.dll")
    parser.add_argument("--entities", type=int, default=5000)
    parser.add_argument("--reverse", action="store_true", help="reverse case order to check order sensitivity")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.entities < 1:
        parser.error("--entities must be positive")
    project = repo / "target/iteration-perf" / ("editor-scene-" + str(os.getpid()))
    sources = project / "content/editor"
    sources.mkdir(parents=True, exist_ok=True)
    (sources / "SceneProbe.cs").write_text(SOURCE, encoding="utf-8")
    rows = [dict(id="e_root", name="Root", order="a0", components={})]
    rows.extend(dict(id=f"e_{i:06}", name=f"Node{i:06}", parent="e_root", order="a0", components={}) for i in range(args.entities))
    report = dict(entities=args.entities + 1, reversed_cases=args.reverse, cases=[])
    with subprocess.Popen(["dotnet", str(args.server.resolve()), "--editor-session", "--parent", str(os.getpid())],
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
        def call(payload):
            process.stdin.write(payload + "\n")
            process.stdin.flush()
            line = process.stdout.readline()
            if not line:
                raise RuntimeError(process.stderr.read())
            result = json.loads(line)
            if not result["accepted"] or any(d.get("severity") == "error" for d in result["diagnostics"]):
                raise RuntimeError(result)
            return result
        try:
            call(json.dumps(dict(project=str(project), authoringRoot="content", operation="catalog")))
            operations = ("snapshot", "hierarchy", "names", "paths")
            for operation in reversed(operations) if args.reverse else operations:
                payload = json.dumps(dict(project=str(project), authoringRoot="content", operation="invoke", command="probe." + operation,
                                          scene=dict(entities=rows)))
                expected = args.entities ** 2 if operation == "hierarchy" else args.entities + 1 if operation == "snapshot" else min(100, args.entities + 1)
                samples = []
                for i in range(9):
                    start = time.perf_counter()
                    result = call(payload)
                    elapsed = (time.perf_counter() - start) * 1000
                    if result["authoring"][0]["args"]["name"] != str(expected):
                        raise RuntimeError("query checksum mismatch")
                    if i >= 2:
                        samples.append(elapsed)
                report["cases"].append(dict(case=operation, p50_ms=statistics.median(samples),
                    p95_ms=sorted(samples)[math.ceil(0.95 * len(samples)) - 1], samples_ms=samples))
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        if process.returncode:
            raise RuntimeError(process.stderr.read())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
