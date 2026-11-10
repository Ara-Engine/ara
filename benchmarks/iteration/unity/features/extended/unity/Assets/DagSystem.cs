using Unity.Burst;
using Unity.Collections;
using Unity.Entities;
using Unity.Jobs;

namespace AraExtended
{
    /// <summary>First dependent stage.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public partial struct JobA : IJobEntity { /// <summary>Fixed logical tick.</summary>
        public uint Tick;
        void Execute(in Value value, ref StageA a) { unchecked { a.Value = MathContract.Mix(value.Word + Tick); } } }
    /// <summary>First independent branch reads A and writes its own column.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public partial struct JobB : IJobEntity { void Execute(in Id id, in StageA a, ref StageB b) { unchecked { b.Value = a.Value * 3 + id.Value; } } }
    /// <summary>Second independent branch reads A and writes its own column.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public partial struct JobC : IJobEntity { /// <summary>Fixed logical tick.</summary>
        public uint Tick;
        void Execute(in Id id, in StageA a, ref StageC c) { unchecked { c.Value = a.Value ^ (id.Value * 5 + Tick); } } }
    /// <summary>Real join of B/C before committing authoritative payload.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public partial struct JobCommit : IJobEntity { void Execute(in StageB b, in StageC c, ref Value value) { unchecked { value.Word = b.Value + c.Value; value.Updates++; } } }

    /// <summary>Retains the query and declares real JobHandle edges for chain or fork/join.</summary>
    [DisableAutoCreation]
    public partial struct DagSystem : ISystem
    {
        EntityQuery query;
        /// <summary>Scheduling topology and fixed tick.</summary>
        public bool Parallel, ForkJoin;
        /// <summary>Fixed logical tick.</summary>
        public uint Tick;
        /// <summary>Builds the common six-component query once.</summary>
        public void OnCreate(ref SystemState state) { query = new EntityQueryBuilder(Allocator.Temp).WithAll<Id>().WithAllRW<Value, StageA>().WithAllRW<StageB, StageC>().Build(ref state); }
        /// <summary>All four scheduling calls, dependency combination and final completion are timed.</summary>
        public void OnUpdate(ref SystemState state)
        {
            var a = new JobA { Tick = Tick }; var b = new JobB(); var c = new JobC { Tick = Tick }; var commit = new JobCommit();
            JobHandle ah = Parallel ? a.ScheduleParallel(query, state.Dependency) : a.Schedule(query, state.Dependency);
            JobHandle bh = Parallel ? b.ScheduleParallel(query, ah) : b.Schedule(query, ah);
            JobHandle ch = Parallel ? c.ScheduleParallel(query, ForkJoin ? ah : bh) : c.Schedule(query, ForkJoin ? ah : bh);
            JobHandle join = ForkJoin ? JobHandle.CombineDependencies(bh, ch) : ch;
            state.Dependency = Parallel ? commit.ScheduleParallel(query, join) : commit.Schedule(query, join);
            state.Dependency.Complete();
        }
        /// <summary>World owns query storage.</summary>
        public void OnDestroy(ref SystemState state) { }
    }
}
