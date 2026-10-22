using System;
using System.Diagnostics;
using System.IO;
using Unity.Burst;
using Unity.Collections;
using Unity.Entities;
using Unity.Jobs.LowLevel.Unsafe;
using UnityEngine;

namespace S8Unity
{
    [Serializable]
    public class ResultRow
    {
        public string setup, workload, unity_version;
        public int count, threads, warmup, frames, job_workers;
        public double p50_ms, p95_ms;
        public double[] ms, sums;
        public float[] samples;
        public bool burst_enabled, development;
    }

    public static class Benchmark
    {
        [RuntimeInitializeOnLoadMethod(RuntimeInitializeLoadType.AfterSceneLoad)]
        static void Start()
        {
#if !UNITY_EDITOR
            try { Run(); Application.Quit(0); }
            catch (Exception e) { UnityEngine.Debug.LogException(e); Application.Quit(1); }
#endif
        }
        public static unsafe void Run()
        {
            string[] args = Environment.GetCommandLineArgs();
            int at = Array.IndexOf(args, "-s8") + 1;
            if (at == 0 || args.Length < at + 7) throw new ArgumentException("-s8 WORKLOAD COUNT THREADS WARMUP FRAMES RESULT_JSON DUMP_OR_DASH");
            string workload = args[at];
            int count = int.Parse(args[at + 1]), threads = int.Parse(args[at + 2]);
            int warmup = int.Parse(args[at + 3]), frames = int.Parse(args[at + 4]);
            JobsUtility.JobWorkerCount = threads > 1 ? Math.Min(threads - 1, JobsUtility.JobWorkerMaximumCount) : 0;
            BurstCompiler.Options.EnableBurstCompilation = true;
            BurstCompiler.Options.EnableBurstSafetyChecks = false;
            using var world = new World("S8");
            var handle = world.CreateSystem<BenchSystem>();
            ref var system = ref world.Unmanaged.GetUnsafeSystemRef<BenchSystem>(handle);
            system.Workload = int.Parse(workload.Substring(1)); system.Parallel = threads > 1;
            system.Configure(world.EntityManager, count);
            var times = new double[frames];
            for (int frame = 0; frame < warmup + frames; frame++)
            {
                long start = Stopwatch.GetTimestamp();
                handle.Update(world.Unmanaged);
                double ms = (Stopwatch.GetTimestamp() - start) * 1000.0 / Stopwatch.Frequency;
                if (frame >= warmup) times[frame - warmup] = ms;
            }
            using var query = world.EntityManager.CreateEntityQuery(typeof(StableId), typeof(Position), typeof(Velocity), typeof(Rotation));
            if (query.CalculateEntityCount() != count) throw new Exception("entity count mismatch");
            using var idData = query.ToComponentDataArray<StableId>(Allocator.Temp);
            using var pData = query.ToComponentDataArray<Position>(Allocator.Temp);
            using var vData = query.ToComponentDataArray<Velocity>(Allocator.Temp);
            using var qData = query.ToComponentDataArray<Rotation>(Allocator.Temp);
            var records = new float[count * 12];
            for (int i = 0; i < count; i++)
            {
                int dst = (idData[i].Value % count) * 12;
                for (int j = 0; j < 4; j++) { records[dst + j] = pData[i].Value[j]; records[dst + 4 + j] = vData[i].Value[j]; records[dst + 8 + j] = qData[i].Value[j]; }
            }
            if (args[at + 6] != "-")
            {
                using var dump = new BinaryWriter(File.Create(args[at + 6]));
                foreach (float f in records) dump.Write(f);
            }
            var sums = new double[12];
            for (int i = 0; i < count; i++) for (int j = 0; j < 12; j++) sums[j] += records[i * 12 + j];
            var samples = new float[64 * 12];
            for (int i = 0; i < 64; i++) Array.Copy(records, i * (count - 1) / 63 * 12, samples, i * 12, 12);
            var sorted = (double[])times.Clone(); Array.Sort(sorted);
            var row = new ResultRow { setup = Application.isEditor ? "unity-editor" : "unity-il2cpp", workload = workload, unity_version = Application.unityVersion,
                count = count, threads = threads, job_workers = JobsUtility.JobWorkerCount, warmup = warmup, frames = frames,
                p50_ms = sorted[(int)Math.Ceiling(frames * 0.50) - 1], p95_ms = sorted[(int)Math.Ceiling(frames * 0.95) - 1],
                ms = times, sums = sums, samples = samples, burst_enabled = BurstCompiler.IsEnabled, development = UnityEngine.Debug.isDebugBuild };
            if (!row.burst_enabled) throw new Exception("Burst was disabled");
            File.WriteAllText(args[at + 5], JsonUtility.ToJson(row, true));
            UnityEngine.Debug.Log($"{row.setup} {workload} n={count} threads={threads} p50={row.p50_ms:F3} p95={row.p95_ms:F3} ms");
        }
    }
}
