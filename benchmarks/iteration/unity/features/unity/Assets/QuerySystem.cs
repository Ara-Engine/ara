using Unity.Burst;
using Unity.Collections;
using Unity.Entities;

namespace AraFeatures
{
    /// <summary>Real Burst job over selected ECS chunks, with declared read/write access.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public partial struct FilterJob : IJobEntity
    {
        /// <summary>Fixed logical input supplied by the fixture's outer tick loop.</summary>
        public uint Tick;

        void Execute(in StableId id, ref FeatureValue value)
        {
            unchecked
            {
                value.Value = value.Value * 1664525u + (1013904223u ^ MathContract.Mix(id.Value + Tick));
                value.Updates++;
            }
        }
    }

    /// <summary>Retains a real archetype-filter query for all warmup and measured steps.</summary>
    [DisableAutoCreation]
    public partial struct QuerySystem : ISystem
    {
        EntityQuery query;
        /// <summary>Whether jobs use parallel chunk scheduling.</summary>
        public bool Parallel;
        /// <summary>Fixed logical tick input; authoritative payload remains in components.</summary>
        public uint Tick;

        /// <summary>Builds the query once; matching archetypes are cached by Entities.</summary>
        public void OnCreate(ref SystemState state)
        {
            query = new EntityQueryBuilder(Allocator.Temp)
                .WithAll<StableId, TagA>().WithAllRW<FeatureValue>().WithNone<TagB>()
                .Build(ref state);
        }

        /// <summary>Dispatches and joins one update; both operations remain timed.</summary>
        public void OnUpdate(ref SystemState state)
        {
            var job = new FilterJob { Tick = Tick };
            state.Dependency = Parallel ? job.ScheduleParallel(query, state.Dependency) : job.Schedule(query, state.Dependency);
            state.Dependency.Complete();
        }

        /// <summary>The owning world disposes its queries.</summary>
        public void OnDestroy(ref SystemState state) { }

        /// <summary>Counts selected entities outside timing and warms the retained query.</summary>
        public int MatchedCount() => query.CalculateEntityCount();
    }
}
