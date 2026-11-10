using System;
using System.Collections.Generic;
using Unity.Burst;
using Unity.Collections;
using Unity.Collections.LowLevel.Unsafe;
using Unity.Entities;

namespace AraExtended
{
    /// <summary>Separate scheduler observation, never an assertion that all workers ran concurrently.</summary>
    [Serializable]
    public sealed class WorkerObservation
    {
        /// <summary>Probe description and deliberate limit on interpretation.</summary>
        public string kind = "separate_1003_entity_ECS_job_probe", caller_observation = "native slots not mapped to managed caller", scope = "does not prove timed-case concurrency";
        /// <summary>Configured native workers and distinct observed native thread slots.</summary>
        public int configured_workers;
        /// <summary>Slots reported by NativeSetThreadIndex in the separate probe.</summary>
        public int[] observed_native_thread_slots;
    }
    /// <summary>Private ECS payload used only by the independent worker probe.</summary>
    public struct ProbeValue : IComponentData { /// <summary>Observable diagnostic work.</summary>
        public uint Value; }
    /// <summary>Writes a unique slot per stable ID, with no shared counter or atomics.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public partial struct ProbeJob : IJobEntity
    {
        /// <summary>One distinct output cell per entity; stable IDs are unique.</summary>
        [NativeDisableParallelForRestriction] public NativeArray<int> Slots;
        /// <summary>Actual native job-thread slot injected by Unity.</summary>
        [NativeSetThreadIndex] public int ThreadSlot;
        void Execute(in Id id, ref ProbeValue value) { Slots[(int)id.Value] = ThreadSlot; value.Value = MathContract.Mix(id.Value + value.Value); }
    }
    /// <summary>Retained query in an entirely separate diagnostic world.</summary>
    [DisableAutoCreation]
    public partial struct ProbeSystem : ISystem
    {
        EntityQuery query;
        /// <summary>Fixed diagnostic output storage and dispatch mode.</summary>
        public NativeArray<int> Slots;
        /// <summary>Parallel query scheduling when enabled.</summary>
        public bool Parallel;
        /// <summary>Builds the diagnostic query once.</summary>
        public void OnCreate(ref SystemState state) { query = new EntityQueryBuilder(Allocator.Temp).WithAll<Id>().WithAllRW<ProbeValue>().Build(ref state); }
        /// <summary>Diagnostic execution only, outside throughput sampling.</summary>
        public void OnUpdate(ref SystemState state)
        {
            var job = new ProbeJob { Slots = Slots };
            state.Dependency = Parallel ? job.ScheduleParallel(query, state.Dependency) : job.Schedule(query, state.Dependency);
            state.Dependency.Complete();
        }
        /// <summary>Caller owns the diagnostic output array.</summary>
        public void OnDestroy(ref SystemState state) { }
    }
    /// <summary>Collects real native slots without adding counters to measured jobs.</summary>
    public static class WorkerProbe
    {
        /// <summary>Observes a small retained ECS job, not the timed workload itself.</summary>
        public static WorkerObservation Observe(bool parallel, int workers)
        {
            using var world = new World("Ara separate worker probe");
            var archetype = world.EntityManager.CreateArchetype(typeof(Id), typeof(ProbeValue));
            using var entities = new NativeArray<Entity>(1003, Allocator.Temp);
            world.EntityManager.CreateEntity(archetype, entities);
            for (int id = 0; id < entities.Length; id++) world.EntityManager.SetComponentData(entities[id], new Id { Value = (uint)id });
            using var slots = new NativeArray<int>(entities.Length, Allocator.TempJob);
            var writableSlots = slots;
            for (int id = 0; id < writableSlots.Length; id++) writableSlots[id] = -1;
            var handle = world.CreateSystem<ProbeSystem>(); ref var system = ref world.Unmanaged.GetUnsafeSystemRef<ProbeSystem>(handle);
            system.Slots = slots; system.Parallel = parallel; handle.Update(world.Unmanaged);
            var observed = new SortedSet<int>();
            foreach (int slot in slots) { if (slot < 0) throw new InvalidOperationException("worker probe skipped entity"); observed.Add(slot); }
            var result = new WorkerObservation { configured_workers = workers, observed_native_thread_slots = new int[observed.Count] };
            observed.CopyTo(result.observed_native_thread_slots); return result;
        }
    }
}
