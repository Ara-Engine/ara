using Unity.Burst;
using Unity.Collections;
using Unity.Entities;
using Unity.Jobs;

namespace AraExtended
{
    /// <summary>One actual structural command per selected stable entity, sorted by stable selection index.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public struct RecordChurn : IJobParallelFor
    {
        /// <summary>Immutable handles selected during untimed setup.</summary>
        [ReadOnly] public NativeArray<Entity> Selected;
        /// <summary>Real structural command writer.</summary>
        public EntityCommandBuffer.ParallelWriter Commands;
        /// <summary>Alternating add/remove operation.</summary>
        public bool Add;
        /// <summary>Records deterministic sort keys; playback remains timed.</summary>
        public void Execute(int index) { if (Add) Commands.AddComponent<TagD>(index, Selected[index]); else Commands.RemoveComponent<TagD>(index, Selected[index]); }
    }
    /// <summary>Updates only the actual post-playback matching archetypes.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public partial struct ChurnUpdate : IJobEntity
    {
        /// <summary>Fixed logical tick.</summary>
        public uint Tick;
        void Execute(in Id id, ref Value value) { value.Word = MathContract.Update(value.Word, id.Value, Tick); value.Updates++; }
    }
    /// <summary>Real ECB creation, recording, join, playback, disposal and retained query update.</summary>
    [DisableAutoCreation]
    public partial struct ChurnSystem : ISystem
    {
        EntityQuery query;
        /// <summary>Selected entity handles are immutable infrastructure, not authoritative component state.</summary>
        public NativeArray<Entity> Selected;
        /// <summary>Fixed logical tick and scheduling mode.</summary>
        public uint Tick;
        /// <summary>Whether the post-playback query schedules in parallel.</summary>
        public bool Parallel;
        /// <summary>Retains the structural filter.</summary>
        public void OnCreate(ref SystemState state) { query = new EntityQueryBuilder(Allocator.Temp).WithAll<Id, TagD>().WithAllRW<Value>().Build(ref state); }
        /// <summary>Measures the complete mutation transaction and payload update.</summary>
        public void OnUpdate(ref SystemState state)
        {
            using var commands = new EntityCommandBuffer(Allocator.TempJob);
            var record = new RecordChurn { Selected = Selected, Commands = commands.AsParallelWriter(), Add = Tick % 2 == 0 };
            var recorded = record.Schedule(Selected.Length, 64, state.Dependency);
            recorded.Complete(); commands.Playback(state.EntityManager);
            var update = new ChurnUpdate { Tick = Tick };
            state.Dependency = Parallel ? update.ScheduleParallel(query, default) : update.Schedule(query, default);
            state.Dependency.Complete();
        }
        /// <summary>Disposes only infrastructure handles; world owns components and queries.</summary>
        public void OnDestroy(ref SystemState state) { if (Selected.IsCreated) Selected.Dispose(); }
    }
}
