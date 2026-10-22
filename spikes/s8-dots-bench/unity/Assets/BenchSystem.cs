using Unity.Collections;
using Unity.Entities;
using Unity.Jobs;

namespace S8Unity
{
    /// Only the simulation/jobs are timed; the empty player has no rendering work.
    [DisableAutoCreation]
    public partial struct BenchSystem : ISystem
    {
        NativeArray<Unity.Mathematics.float4> p, v;
        NativeArray<int> offsets, ids, cursor;
        NativeArray<Entity> slots;
        EntityArchetype archetype;
        public int Workload;
        public bool Parallel;
        public uint Frame;

        public void OnCreate(ref SystemState state) { }
        public void Configure(EntityManager manager, int count)
        {
            p = new NativeArray<Unity.Mathematics.float4>(count, Allocator.Persistent);
            v = new NativeArray<Unity.Mathematics.float4>(count, Allocator.Persistent);
            offsets = new NativeArray<int>(4097, Allocator.Persistent);
            ids = new NativeArray<int>(count, Allocator.Persistent);
            cursor = new NativeArray<int>(4096, Allocator.Persistent);
            slots = new NativeArray<Entity>(count, Allocator.Persistent);
            archetype = manager.CreateArchetype(typeof(Position), typeof(Velocity), typeof(Rotation), typeof(StableId));
            manager.CreateEntity(archetype, slots);
            for (int i = 0; i < count; i++)
            {
                MathContract.Initial((uint)i, out var pi, out var vi, out var qi);
                manager.SetComponentData(slots[i], new StableId { Value = i });
                manager.SetComponentData(slots[i], new Position { Value = pi });
                manager.SetComponentData(slots[i], new Velocity { Value = vi });
                manager.SetComponentData(slots[i], new Rotation { Value = qi });
            }
        }
        public void OnDestroy(ref SystemState state)
        {
            p.Dispose(); v.Dispose(); offsets.Dispose(); ids.Dispose(); cursor.Dispose(); slots.Dispose();
        }
        public void OnUpdate(ref SystemState state)
        {
            if (Workload == 1)
            {
                var job = new TransformJob();
                state.Dependency = Parallel ? job.ScheduleParallel(state.Dependency) : job.Schedule(state.Dependency);
                state.Dependency.Complete();
            }
            else if (Workload == 2)
            {
                var snapshot = new SnapshotJob { P = p, V = v };
                var ready = Parallel ? snapshot.ScheduleParallel(state.Dependency) : snapshot.Schedule(state.Dependency);
                var grid = new GridJob { P = p, Offsets = offsets, Ids = ids, Cursor = cursor };
                var built = grid.Schedule(ready);
                var boids = new BoidJob { P = p, V = v, Offsets = offsets, Ids = ids };
                state.Dependency = Parallel ? boids.ScheduleParallel(built) : boids.Schedule(built);
                state.Dependency.Complete();
            }
            else if (Workload == 3)
            {
                var job = new ParticleJob();
                state.Dependency = Parallel ? job.ScheduleParallel(state.Dependency) : job.Schedule(state.Dependency);
                state.Dependency.Complete();
            }
            else if (Workload == 4)
            {
                using var ecb = new EntityCommandBuffer(Allocator.TempJob);
                var job = new ChurnJob { Frame = Frame, Archetype = archetype, Commands = ecb.AsParallelWriter(), Slots = slots };
                // batch=10000 is one job for single-thread mode; batch=256 distributes recording.
                job.Schedule(10000, Parallel ? 256 : 10000, state.Dependency).Complete();
                ecb.Playback(state.EntityManager);
                var collect = new CollectSlotsJob { Slots = slots };
                state.Dependency = Parallel ? collect.ScheduleParallel(state.Dependency) : collect.Schedule(state.Dependency);
                state.Dependency.Complete();
            }
            Frame++;
        }
    }
}
