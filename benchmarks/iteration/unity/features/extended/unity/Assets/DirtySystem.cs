using Unity.Burst;
using Unity.Collections;
using Unity.Entities;
using Unity.Burst.Intrinsics;

namespace AraExtended
{
    /// <summary>Reads IDs first; only requests writable epoch storage for selected chunks.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public struct MarkEpoch : IJobChunk
    {
        /// <summary>Read-only identity column.</summary>
        [ReadOnly] public ComponentTypeHandle<Id> Ids;
        /// <summary>Changed-version stamp is applied only when this handle is actually accessed.</summary>
        public ComponentTypeHandle<Epoch> Epochs;
        /// <summary>Fixed logical inputs.</summary>
        public uint Tick, Count, Percent;
        /// <summary>Rotating interval when true, otherwise ID modulo distribution.</summary>
        public bool Clustered;
        /// <summary>No enableable components are present in this fixture.</summary>
        public void Execute(in ArchetypeChunk chunk, int chunkIndex, bool useEnabledMask, in v128 enabledMask)
        {
            var ids = chunk.GetNativeArray(ref Ids);
            bool selected = false;
            for (int i = 0; i < ids.Length; i++)
                if (MathContract.Selected(ids[i].Value, Tick, Count, Percent, Clustered)) { selected = true; break; }
            if (!selected) return;
            var epochs = chunk.GetNativeArray(ref Epochs);
            for (int i = 0; i < ids.Length; i++)
                if (MathContract.Selected(ids[i].Value, Tick, Count, Percent, Clustered)) epochs[i] = new Epoch { Value = Tick + 1 };
        }
    }

    /// <summary>Chunk change filter narrows candidates; explicit epoch guarantees identical entity semantics.</summary>
    [BurstCompile(CompileSynchronously = true)]
    public partial struct ConsumeEpoch : IJobEntity
    {
        /// <summary>Fixed tick.</summary>
        public uint Tick;
        void Execute(in Id id, in Epoch epoch, ref Value value)
        {
            if (epoch.Value != Tick + 1) return;
            value.Word = MathContract.Update(value.Word, id.Value, Tick);
            value.Updates++;
        }
    }

    /// <summary>Retained marking and actual changed-version queries, joined within each timed update.</summary>
    [DisableAutoCreation]
    public partial struct DirtySystem : ISystem
    {
        EntityQuery all, changed;
        ComponentTypeHandle<Id> ids;
        ComponentTypeHandle<Epoch> epochs;
        /// <summary>Fixed step inputs.</summary>
        public uint Tick, Count, Percent;
        /// <summary>Scheduling and selection modes.</summary>
        public bool Parallel, Clustered;
        /// <summary>Separate replay only: count real filtered candidates after marking completes.</summary>
        public bool Diagnostics;
        /// <summary>Last replay step's candidate visits and explicit logical matches.</summary>
        public long Candidates, Logical;
        /// <summary>Registers both queries so the system supplies their required change versions each update.</summary>
        public void OnCreate(ref SystemState state)
        {
            all = new EntityQueryBuilder(Allocator.Temp).WithAll<Id>().WithAllRW<Epoch>().Build(ref state);
            changed = new EntityQueryBuilder(Allocator.Temp).WithAll<Id, Epoch>().WithAllRW<Value>().Build(ref state);
            changed.SetChangedVersionFilter(ComponentType.ReadOnly<Epoch>());
            ids = state.GetComponentTypeHandle<Id>(true);
            epochs = state.GetComponentTypeHandle<Epoch>();
        }
        /// <summary>Marking, scheduling, change filtering and completion are all measured.</summary>
        public void OnUpdate(ref SystemState state)
        {
            ids.Update(ref state); epochs.Update(ref state);
            var mark = new MarkEpoch { Ids = ids, Epochs = epochs,
                Tick = Tick, Count = Count, Percent = Percent, Clustered = Clustered };
            var marked = Parallel ? mark.ScheduleParallel(all, state.Dependency) : mark.Schedule(all, state.Dependency);
            if (Diagnostics)
            {
                marked.Complete();
                using var epochs = changed.ToComponentDataArray<Epoch>(Allocator.Temp);
                Candidates = epochs.Length; Logical = 0;
                foreach (var epoch in epochs) if (epoch.Value == Tick + 1) Logical++;
            }
            var consume = new ConsumeEpoch { Tick = Tick };
            state.Dependency = Parallel ? consume.ScheduleParallel(changed, marked) : consume.Schedule(changed, marked);
            state.Dependency.Complete();
        }
        /// <summary>The owning world disposes retained queries.</summary>
        public void OnDestroy(ref SystemState state) { }
    }
}
