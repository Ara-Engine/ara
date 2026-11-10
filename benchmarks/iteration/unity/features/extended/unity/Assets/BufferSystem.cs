using Unity.Burst;
using Unity.Collections;
using Unity.Entities;

namespace AraExtended
{
    /// <summary>Uses actual ECS buffer resize/trim APIs; initialization and live-word folding remain timed.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public partial struct BufferJob : IJobEntity
    {
        /// <summary>Fixed step and selected fraction.</summary>
        public uint Tick, Percent;
        /// <summary>Grow/shrink and capacity policy.</summary>
        public bool Resize, Trim;
        void Execute(in Id id, ref Value value, ref DynamicBuffer<Word> words)
        {
            unchecked
            {
                if (Resize && id.Value % 100 < Percent)
                {
                    int length = Tick % 2 == 0 ? 64 : 4, previous = words.Length;
                    words.ResizeUninitialized(length);
                    for (int i = previous; i < length; i++) words[i] = new Word { Value = MathContract.InitialWord(id.Value, i) };
                    if (Trim) words.TrimExcess();
                }
                uint sum = 0;
                for (int i = 0; i < words.Length; i++)
                {
                    uint word = MathContract.Update(words[i].Value, id.Value * 64 + (uint)i, Tick);
                    words[i] = new Word { Value = word }; sum += word;
                }
                value.Word += sum; value.Updates++;
            }
        }
    }
    /// <summary>Retained buffer query with timed scheduling, resize work and completion.</summary>
    [DisableAutoCreation]
    public partial struct BufferSystem : ISystem
    {
        EntityQuery query;
        /// <summary>Scheduling and capacity modes.</summary>
        public bool Parallel, Resize, Trim;
        /// <summary>Fixed step inputs.</summary>
        public uint Tick, Percent;
        /// <summary>Builds one actual buffer query.</summary>
        public void OnCreate(ref SystemState state) { query = new EntityQueryBuilder(Allocator.Temp).WithAll<Id>().WithAllRW<Value, Word>().Build(ref state); }
        /// <summary>Includes all live payload operations.</summary>
        public void OnUpdate(ref SystemState state)
        {
            var job = new BufferJob { Tick = Tick, Percent = Percent, Resize = Resize, Trim = Trim };
            state.Dependency = Parallel ? job.ScheduleParallel(query, state.Dependency) : job.Schedule(query, state.Dependency);
            state.Dependency.Complete();
        }
        /// <summary>World owns query storage.</summary>
        public void OnDestroy(ref SystemState state) { }
    }
}
