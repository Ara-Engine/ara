using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using Unity.Burst;
using Unity.Collections;
using Unity.Entities;
using Unity.Jobs.LowLevel.Unsafe;
using UnityEngine;

namespace AraFeatures
{
    /// <summary>Measured samples and explicit retained-query fixture settings.</summary>
    [Serializable]
    public sealed class FeatureResult
    {
        /// <summary>Execution lane and implemented feature name.</summary>
        public string lane, workload;
        /// <summary>Entity/archetype counts and entities selected by the cached filter.</summary>
        public int count, archetypes, matched_entities;
        /// <summary>Requested concurrency, actual workers, step counts and Ara-only batch argument.</summary>
        public int requested_threads, job_workers, max_kernel_runners, warmup, frames, batch;
        /// <summary>Warm-process percentiles and every recorded step duration.</summary>
        public double p50_ms, p95_ms;
        /// <summary>Raw measured step durations in milliseconds.</summary>
        public double[] ms;
        /// <summary>Whether Burst was enabled and complete ID/tag/update checks passed.</summary>
        public bool burst_enabled, full_ids_tags_updates_checked;
        /// <summary>Canonical binary state length.</summary>
        public long state_bytes;
    }

    /// <summary>Runs only the real retained-query feature in an isolated batch world.</summary>
    public static class QueryBenchmark
    {
        static string OutputPath(string raw)
        {
            string root = Directory.GetParent(Application.dataPath).FullName + Path.DirectorySeparatorChar;
            string path = Path.GetFullPath(raw);
            if (!path.StartsWith(root, StringComparison.Ordinal)) throw new ArgumentException("output must remain in scratch project");
            if (File.Exists(path)) throw new IOException("output exists; use a fresh run");
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            return path;
        }

        static void Configure(EntityManager manager, int count, int archetypes)
        {
            ComponentType[] tags = { ComponentType.ReadWrite<TagA>(), ComponentType.ReadWrite<TagB>(),
                ComponentType.ReadWrite<TagC>(), ComponentType.ReadWrite<TagD>(),
                ComponentType.ReadWrite<TagE>(), ComponentType.ReadWrite<TagF>() };
            for (int mask = 0; mask < archetypes; mask++)
            {
                var types = new List<ComponentType> { ComponentType.ReadWrite<StableId>(), ComponentType.ReadWrite<FeatureValue>() };
                for (int bit = 0; bit < tags.Length; bit++) if ((mask & (1 << bit)) != 0) types.Add(tags[bit]);
                EntityArchetype archetype = manager.CreateArchetype(types.ToArray());
                int groupCount = (count - 1 - mask) / archetypes + 1;
                using var entities = new NativeArray<Entity>(groupCount, Allocator.Temp);
                manager.CreateEntity(archetype, entities);
                for (int i = 0; i < groupCount; i++)
                {
                    uint id = (uint)(mask + i * archetypes);
                    manager.SetComponentData(entities[i], new StableId { Value = id });
                    manager.SetComponentData(entities[i], new FeatureValue { Value = MathContract.Mix(id ^ 0x6d2b79f5u) });
                }
            }
        }

        /// <summary>Initialization/export are untimed; each scheduled update and completion are timed.</summary>
        public static void Run()
        {
            string[] args = Environment.GetCommandLineArgs();
            int at = Array.IndexOf(args, "-ara-feature") + 1;
            if (at == 0 || args.Length < at + 9 || args[at] != "query_filter")
                throw new ArgumentException("-ara-feature query_filter COUNT THREADS WARMUP FRAMES RESULT DUMP ARCHETYPES BATCH");
            int count = int.Parse(args[at + 1]), threads = int.Parse(args[at + 2]);
            int warmup = int.Parse(args[at + 3]), frames = int.Parse(args[at + 4]);
            int archetypes = int.Parse(args[at + 7]), batch = int.Parse(args[at + 8]);
            int steps = checked(warmup + frames);
            if (count < 64 || threads < 1 || warmup < 0 || frames < 1 || batch < 1 || (archetypes != 8 && archetypes != 64))
                throw new ArgumentException("invalid feature parameters");
            string result = OutputPath(args[at + 5]), dump = OutputPath(args[at + 6]);
            if (result == dump) throw new ArgumentException("result/dump paths must differ");
            JobsUtility.JobWorkerCount = threads == 1 ? 0 : Math.Min(threads - 1, JobsUtility.JobWorkerMaximumCount);
            using var world = new World("Ara query-filter feature");
            var handle = world.CreateSystem<QuerySystem>();
            ref var system = ref world.Unmanaged.GetUnsafeSystemRef<QuerySystem>(handle);
            system.Parallel = threads > 1;
            Configure(world.EntityManager, count, archetypes);
            int matched = system.MatchedCount(), expected = 0;
            for (int id = 0; id < count; id++) if ((id & 1) != 0 && (id & 2) == 0) expected++;
            if (matched != expected) throw new InvalidOperationException("query selected incorrect count");
            var samples = new double[frames];
            for (int tick = 0; tick < steps; tick++)
            {
                system.Tick = (uint)tick;
                long start = Stopwatch.GetTimestamp();
                handle.Update(world.Unmanaged);
                double elapsed = (Stopwatch.GetTimestamp() - start) * 1000.0 / Stopwatch.Frequency;
                if (tick >= warmup) samples[tick - warmup] = elapsed;
            }
            long stateBytes = StateWriter.Write(world.EntityManager, dump, count, archetypes, (uint)steps);
            var sorted = (double[])samples.Clone(); Array.Sort(sorted);
            var row = new FeatureResult { lane = "unity-editor-burst-query-filter", workload = "query_filter",
                count = count, archetypes = archetypes, matched_entities = matched, requested_threads = threads,
                job_workers = JobsUtility.JobWorkerCount, max_kernel_runners = JobsUtility.JobWorkerCount + 1,
                warmup = warmup, frames = frames, batch = batch, ms = samples,
                p50_ms = sorted[(int)Math.Ceiling(frames * 0.50) - 1], p95_ms = sorted[(int)Math.Ceiling(frames * 0.95) - 1],
                burst_enabled = BurstCompiler.IsEnabled, full_ids_tags_updates_checked = true, state_bytes = stateBytes };
            File.WriteAllText(result, JsonUtility.ToJson(row, true));
            UnityEngine.Debug.Log($"query_filter n={count} archetypes={archetypes} p50={row.p50_ms:F4} p95={row.p95_ms:F4} ms");
        }
    }
}
