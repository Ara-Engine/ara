"""Fresh S8 control lane; original spike kernels and MacBatch remain frozen."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import sys

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "features"))
import project as features
import inputs
import recovery

ROOT = features.ROOT
BASE = features.BASE
PROJECT = ROOT / "target/perf-scale/us8"
MARKER = ".ara-s8-current.json"
SOURCE = ROOT / "spikes/s8-dots-bench/unity/Assets"
COUNTS = {"w1": 1_000_000, "w2": 20_000, "w3": 1_000_000, "w4": 100_000}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def templates():
    files = [p for p in SOURCE.rglob("*") if p.is_file()]
    files += [p for p in HERE.rglob("*") if p.is_file() and p.suffix in (".py", ".cs")]
    files += [HERE.parent / "MacBatch.cs", HERE.parent / "bench.py", HERE.parent / "dump_hash.rs"]
    files += [features.HERE / name for name in ("inputs.py", "recovery.py", "project.py", "recover.py", "monitor.py")]
    files += [features.HERE / "editor/PreferenceRecovery.cs"]
    return {str(p.relative_to(ROOT)): sha(p) for p in sorted(files)}


def instrument_export(source):
    """Insert identity proof only after the original measured loop/query copies."""
    seam = '            var records = new float[count * 12];'
    if source.count(seam) != 1: raise ValueError("S8 export seam changed; review adapter")
    code = '''            // Identity/component proof is outside every measured frame.
            using var identities = world.EntityManager.CreateEntityQuery(typeof(StableId));
            if (identities.CalculateEntityCount() != count)
                throw new Exception("unexpected identity count or missing required components");
            var exactIds = new int[count];
            var seenIds = new bool[count];
            for (int i = 0; i < count; i++)
            {
                int id = idData[i].Value, slot = id % count;
                if (id < 0 || seenIds[slot]) throw new Exception("duplicate/negative logical ID");
                seenIds[slot] = true;
                int expected = slot;
                if (workload == "w4")
                {
                    int group = slot / 10000;
                    if (warmup + frames > group)
                    {
                        int last = warmup + frames - 1;
                        int frame = last - (last - group) % 10;
                        expected = checked(100000 + frame * 10000 + slot % 10000);
                    }
                }
                if (id != expected) throw new Exception("exact StableId epoch mismatch");
                exactIds[slot] = id;
            }
            if (Array.Exists(seenIds, seen => !seen)) throw new Exception("missing logical ID");
            if (args[at + 6] != "-")
            {
                using var ids = new BinaryWriter(File.Create(args[at + 6] + ".ids"));
                foreach (int id in exactIds) ids.Write(id);
            }
'''
    return source.replace(seam, code + seam)


def prepare(args):
    root = BASE.scratch_path(args.project)
    if root.exists() and any(root.iterdir()) and not (root / MARKER).is_file():
        raise ValueError("refusing an existing project without current S8 marker")
    if not BASE.EDITOR.is_file(): raise ValueError("pinned Unity is missing")
    with (BASE.EDITOR.parents[1] / "Info.plist").open("rb") as stream:
        version = str(plistlib.load(stream)["CFBundleShortVersionString"]).removeprefix("Unity version ")
    if version != "6000.4.5f1": raise ValueError("unexpected pinned Editor version")
    inputs.sync_assets(SOURCE, root / "Assets", ["Editor/MacBatch.cs", "Editor/PreferenceRecovery.cs"])
    original = (HERE.parent / "MacBatch.cs").read_text()
    wrapper = recovery.adapt_wrapper(original)
    inputs.write_if_changed(root / "Assets/Editor/MacBatch.cs", wrapper.encode())
    inputs.write_if_changed(root / "Assets/Editor/PreferenceRecovery.cs", (features.HERE / "editor/PreferenceRecovery.cs").read_bytes())
    inputs.write_if_changed(root / "Assets/Benchmark.cs", instrument_export((SOURCE / "Benchmark.cs").read_text()).encode())
    assembly = json.loads((SOURCE / "Editor/S8.Editor.asmdef").read_text())
    if "Unity.Burst" not in assembly["references"]: assembly["references"].append("Unity.Burst")
    inputs.write_if_changed(root / "Assets/Editor/S8.Editor.asmdef", (json.dumps(assembly, indent=2) + "\n").encode())
    inputs.write_if_changed(root / "Packages/manifest.json", (json.dumps({"dependencies": BASE.PACKAGES}, indent=2) + "\n").encode())
    version_path = root / "ProjectSettings/ProjectVersion.txt"
    if not version_path.exists() or "m_EditorVersion: 6000.4.5f1" not in version_path.read_text().splitlines():
        inputs.write_if_changed(version_path, b"m_EditorVersion: 6000.4.5f1\n")
    BASE.write_json(root / MARKER, {"fixture": "s8-current-v1", "repository_root": str(ROOT),
        "editor": str(BASE.EDITOR), "requested_packages": BASE.PACKAGES, "input_manifest_version": 1,
        "template_inputs": templates(), "prepared_inputs": inputs.prepared(root),
        "numerical_kernels_changed": False, "export_adapter": "exact IDs/components after measured loop"})
    recovery.register(root)
    print(json.dumps({"project": str(root), "launched": False}))


def prepared(args):
    proof = inputs.check(BASE.scratch_path(args.project), MARKER, templates(), str(BASE.EDITOR), BASE.PACKAGES)
    if proof["input_manifest"]["fixture"] != "s8-current-v1": raise ValueError("wrong S8 fixture family")
    return proof


prepared_manifest = prepared


def recovery_temp(args):
    return ROOT / "target/perf-scale/us8-t"


def config(args):
    return {key: getattr(args, key) for key in ("workload", "count", "runners", "warmup", "frames", "batch", "variant", "smoke")}


def budget(args):
    threads = 1 if args.runners == 1 else (15 if args.workload == "w4" else 14)
    return {"ara": {"argument_threads": threads, **BASE.ara_runners(args.workload, threads)},
            "unity": {"requested_runners": args.runners, "expected_job_workers": args.runners - 1,
                      "caller_may_participate": True, "max_kernel_runners": args.runners}}


def paths(args, engine):
    root = BASE.scratch_path(args.project)
    stem = f"{args.workload}_{args.count}_r{args.runners}_{engine}" + ("_smoke" if args.smoke else "")
    folder = root / "results" / args.run
    return folder / (stem + ".json"), folder / (stem + ".bin"), folder / (stem + ".log")


def command(args, engine, binary=None):
    prepared(args)
    result, dump, log = paths(args, engine)
    env = dict(os.environ)
    if engine == "ara":
        argv = ["/usr/bin/nice", "-n", "10", str(binary), args.workload, str(args.count),
                str(budget(args)["ara"]["argument_threads"]), str(args.warmup), str(args.frames),
                args.variant, str(args.batch), str(result)]
    else:
        root = BASE.scratch_path(args.project)
        env.update(UPM_CACHE_ROOT=str(root / "upm-cache"), UPM_GIT_LFS_CACHE_PATH=str(root / "upm-cache/git-lfs"),
                   TMPDIR=str(ROOT / "target/perf-scale/us8-t"))
        argv = ["/usr/bin/nice", "-n", "10", str(BASE.EDITOR), "-batchmode", "-nographics",
                "-projectPath", str(root), "-job-worker-count", "14", "-executeMethod", "AraIteration.MacBatch.Run",
                "-logFile", str(log), "-s8", args.workload, str(args.count), str(args.runners),
                str(args.warmup), str(args.frames), str(result), str(dump)]
    return argv, env, result, dump, log
