using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using Unity.Burst;
using Unity.Collections;
using Unity.Entities;
using Unity.Jobs.LowLevel.Unsafe;
using UnityEngine;

namespace AraExtended
{
    /// <summary>Extended native ECS API fixture evidence; caller concurrency is an upper bound.</summary>
    [Serializable]
    public sealed class Result
    {
        /// <summary>Engine lane and exact case.</summary>
        public string lane, workload;
        /// <summary>Entity count and zero for the unused static-tag sweep.</summary>
        public int count, archetypes;
        /// <summary>Requested runner budget, actual pool workers, and theoretical caller-plus-pool ceiling.</summary>
        public int requested_threads, job_workers, max_kernel_runners, warmup, frames, batch;
        /// <summary>Complete timing samples.</summary>
        public double[] ms;
        /// <summary>Warm-process percentiles.</summary>
        public double p50_ms, p95_ms;
        /// <summary>Controlled Burst and full identity/component/epoch/update audit.</summary>
        public bool burst_enabled, full_ids_tags_updates_checked;
        /// <summary>Version-one full state size.</summary>
        public long state_bytes;
        /// <summary>Exact immutable case inputs.</summary>
        public Config config;
        /// <summary>Actual installed component/buffer layout and post-run capacities.</summary>
        public Layout layout;
        /// <summary>Separate small ECS job's observed native slots; never timed-case concurrency.</summary>
        public WorkerObservation worker_probe;
        /// <summary>Source-defined structural command total; not an allocator counter.</summary>
        public long expected_structural_commands;
        /// <summary>Per-step candidate/logical visits from a separate diagnostic replay, never timed samples.</summary>
        public long[] diagnostic_candidate_visits, diagnostic_logical_updates;
        /// <summary>Returned consumer candidates, not total rows scanned by filter or marker.</summary>
        public string dirty_counter_scope = "returned consumer candidates; not total rows scanned";
    }

    /// <summary>Five actual Entities/Jobs/Burst workloads in a private batch-only scratch project.</summary>
    public static class ExtendedBenchmark
    {
        static readonly string[] Cases = { "dirty_epoch", "archetype_churn", "dependency_dag", "buffer_fixed", "buffer_resize" };
        static string Output(string raw)
        {
            string root = Directory.GetParent(Application.dataPath).FullName + Path.DirectorySeparatorChar;
            string path = Path.GetFullPath(raw);
            if (!path.StartsWith(root, StringComparison.Ordinal) || File.Exists(path)) throw new IOException("unsafe or existing output");
            Directory.CreateDirectory(Path.GetDirectoryName(path)); return path;
        }

        static NativeArray<Entity> Configure(EntityManager manager, string name, int count, Config config)
        {
            var types = new List<ComponentType> { typeof(Id), typeof(Value) };
            if (name == "dirty_epoch") types.Add(typeof(Epoch));
            if (name == "dependency_dag") { types.Add(typeof(StageA)); types.Add(typeof(StageB)); types.Add(typeof(StageC)); }
            if (name.StartsWith("buffer_", StringComparison.Ordinal)) types.Add(typeof(Word));
            EntityArchetype archetype = manager.CreateArchetype(types.ToArray());
            using var entities = new NativeArray<Entity>(count, Allocator.Temp);
            manager.CreateEntity(archetype, entities);
            var selected = new List<Entity>();
            for (int id = 0; id < count; id++)
            {
                Entity entity = entities[id]; manager.SetComponentData(entity, new Id { Value = (uint)id });
                manager.SetComponentData(entity, new Value { Word = MathContract.Mix((uint)id ^ 0x6d2b79f5u) });
                if (name == "archetype_churn" && id % 100 < config.percent) selected.Add(entity);
                if (name.StartsWith("buffer_", StringComparison.Ordinal))
                {
                    var words = manager.GetBuffer<Word>(entity); int length = name == "buffer_fixed" ? config.length : 4;
                    words.ResizeUninitialized(length);
                    for (int i = 0; i < length; i++) words[i] = new Word { Value = MathContract.InitialWord((uint)id, i) };
                }
            }
            return new NativeArray<Entity>(selected.ToArray(), Allocator.Persistent);
        }

        static SystemHandle Create(World world, string name, bool parallel, Config config, int count)
        {
            NativeArray<Entity> selected = Configure(world.EntityManager, name, count, config);
            if (name == "dirty_epoch")
            {
                selected.Dispose(); var h = world.CreateSystem<DirtySystem>();
                ref var system = ref world.Unmanaged.GetUnsafeSystemRef<DirtySystem>(h);
                system.Parallel = parallel; system.Count = (uint)count; system.Percent = (uint)config.percent; system.Clustered = config.distribution == "clustered"; return h;
            }
            if (name == "archetype_churn")
            {
                var h = world.CreateSystem<ChurnSystem>(); ref var system = ref world.Unmanaged.GetUnsafeSystemRef<ChurnSystem>(h);
                system.Parallel = parallel; system.Selected = selected; return h;
            }
            selected.Dispose();
            if (name == "dependency_dag")
            {
                var h = world.CreateSystem<DagSystem>(); ref var system = ref world.Unmanaged.GetUnsafeSystemRef<DagSystem>(h);
                system.Parallel = parallel; system.ForkJoin = config.fork_join; return h;
            }
            else
            {
                var h = world.CreateSystem<BufferSystem>(); ref var system = ref world.Unmanaged.GetUnsafeSystemRef<BufferSystem>(h);
                system.Parallel = parallel; system.Resize = name == "buffer_resize"; system.Trim = config.trim; system.Percent = (uint)config.percent; return h;
            }
        }

        static void Tick(World world, SystemHandle handle, string name, uint tick)
        {
            switch (name)
            {
                case "dirty_epoch": world.Unmanaged.GetUnsafeSystemRef<DirtySystem>(handle).Tick = tick; break;
                case "archetype_churn": world.Unmanaged.GetUnsafeSystemRef<ChurnSystem>(handle).Tick = tick; break;
                case "dependency_dag": world.Unmanaged.GetUnsafeSystemRef<DagSystem>(handle).Tick = tick; break;
                default: world.Unmanaged.GetUnsafeSystemRef<BufferSystem>(handle).Tick = tick; break;
            }
            handle.Update(world.Unmanaged);
        }

        /// <summary>Only fixed input assignment, all logical work, scheduling and completion are timed.</summary>
        public static void Run()
        {
            string[] args = Environment.GetCommandLineArgs(); int at = Array.IndexOf(args, "-ara-feature") + 1;
            if (at == 0 || args.Length < at + 9) throw new ArgumentException("-ara-feature CASE COUNT RUNNERS WARMUP FRAMES RESULT DUMP CONFIG_JSON BATCH");
            string name = args[at]; int caseId = Array.IndexOf(Cases, name) + 2;
            int count = int.Parse(args[at + 1]), runners = int.Parse(args[at + 2]), warmup = int.Parse(args[at + 3]), frames = int.Parse(args[at + 4]), batch = int.Parse(args[at + 8]);
            int steps = checked(warmup + frames);
            var config = JsonUtility.FromJson<Config>(File.ReadAllText(args[at + 7]));
            if (caseId < 2 || count < 64 || runners < 1 || warmup < 0 || frames < 1 || batch < 1 || config.percent < 0 || config.percent > 100
                || (config.distribution != "spread" && config.distribution != "clustered") || Array.IndexOf(new[] { 0, 4, 16, 64 }, config.length) < 0) throw new ArgumentException("invalid extended fixture inputs");
            string result = Output(args[at + 5]), dump = Output(args[at + 6]);
            if (result == dump) throw new ArgumentException("result/dump paths differ");
            JobsUtility.JobWorkerCount = runners == 1 ? 0 : Math.Min(runners - 1, JobsUtility.JobWorkerMaximumCount);
            using var world = new World("Ara extended ECS fixture");
            SystemHandle handle = Create(world, name, runners > 1, config, count);
            var samples = new double[frames];
            for (uint tick = 0; tick < steps; tick++)
            {
                long started = Stopwatch.GetTimestamp(); Tick(world, handle, name, tick);
                double elapsed = (Stopwatch.GetTimestamp() - started) * 1000.0 / Stopwatch.Frequency;
                if (tick >= warmup) samples[tick - warmup] = elapsed;
            }
            long size = StateWriter.Write(world.EntityManager, dump, name, caseId, count, (uint)steps, config, out Layout layout);
            var sorted = (double[])samples.Clone(); Array.Sort(sorted);
            long selected = 0; if (name == "archetype_churn") for (int id = 0; id < count; id++) if (id % 100 < config.percent) selected++;
            var row = new Result { lane = "unity-editor-burst-extended", workload = name, count = count, archetypes = 0,
                requested_threads = runners, job_workers = JobsUtility.JobWorkerCount, max_kernel_runners = JobsUtility.JobWorkerCount + 1,
                warmup = warmup, frames = frames, batch = batch, config = config, layout = layout, ms = samples,
                p50_ms = sorted[(int)Math.Ceiling(frames * .50) - 1], p95_ms = sorted[(int)Math.Ceiling(frames * .95) - 1],
                burst_enabled = BurstCompiler.IsEnabled, full_ids_tags_updates_checked = true, state_bytes = size,
                expected_structural_commands = selected * steps };
            row.worker_probe = WorkerProbe.Observe(runners > 1, JobsUtility.JobWorkerCount);
            if (name == "dirty_epoch")
            {
                using var diagnostic = new World("Ara untimed dirty candidate replay");
                var h = Create(diagnostic, name, false, config, count);
                ref var observer = ref diagnostic.Unmanaged.GetUnsafeSystemRef<DirtySystem>(h); observer.Diagnostics = true;
                row.diagnostic_candidate_visits = new long[steps]; row.diagnostic_logical_updates = new long[steps];
                for (uint tick = 0; tick < steps; tick++) { Tick(diagnostic, h, name, tick); row.diagnostic_candidate_visits[tick] = observer.Candidates; row.diagnostic_logical_updates[tick] = observer.Logical; }
                string diagnosticPath = dump + ".diagnostic.bin";
                StateWriter.Write(diagnostic.EntityManager, diagnosticPath, name, caseId, count, (uint)steps, config, out _);
                if (!FileBytesEqual(dump, diagnosticPath)) throw new InvalidOperationException("diagnostic replay differs from measured full state");
            }
            File.WriteAllText(result, JsonUtility.ToJson(row, true));
            UnityEngine.Debug.Log($"{name} n={count} p50={row.p50_ms:F4} ms (acceptance requires external full oracle validation)");
        }

        static bool FileBytesEqual(string a, string b)
        {
            using var first = File.OpenRead(a); using var second = File.OpenRead(b);
            if (first.Length != second.Length) return false;
            int value; while ((value = first.ReadByte()) >= 0) if (value != second.ReadByte()) return false;
            return true;
        }
    }
}
