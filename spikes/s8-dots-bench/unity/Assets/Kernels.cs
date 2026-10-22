using Unity.Burst;
using Unity.Collections;
using Unity.Collections.LowLevel.Unsafe;
using Unity.Entities;
using Unity.Jobs;
using Unity.Mathematics;

namespace S8Unity
{
    public struct Position : IComponentData { public float4 Value; }
    public struct Velocity : IComponentData { public float4 Value; }
    public struct Rotation : IComponentData { public float4 Value; }
    public struct StableId : IComponentData { public int Value; }

    public static class MathContract
    {
        public const float Dt = 1f / 60f;
        static float Unit(uint id, uint salt)
        {
            uint x = id ^ salt;
            x = unchecked((x ^ (x >> 16)) * 0x7feb352du);
            x = unchecked((x ^ (x >> 15)) * 0x846ca68bu);
            return ((x ^ (x >> 16)) & 65535u) / 65536f;
        }
        public static void Initial(uint id, out float4 p, out float4 v, out float4 q)
        {
            p = new float4(Unit(id, 2891336453u), Unit(id, 1181783497u), Unit(id, 425950093u), 0) * 64f - 32f;
            p.w = 1;
            v = new float4(Unit(id, 97), Unit(id, 193), Unit(id, 389), 0) * 2f - 1f;
            v.w = 0; q = new float4(0, 0, 0, 1);
        }
        public static int3 Cell(float4 p) => math.clamp((int3)math.floor((p.xyz + 32f) * 0.25f), 0, 15);
        public static int Key(int3 c) => c.x + 16 * (c.y + 16 * c.z);
    }

    [BurstCompile(CompileSynchronously = true, FloatMode = FloatMode.Strict, FloatPrecision = FloatPrecision.Standard)]
    public partial struct TransformJob : IJobEntity
    {
        void Execute(ref Position position, in Velocity velocity, ref Rotation rotation)
        {
            position.Value += velocity.Value * MathContract.Dt;
            float4 a = velocity.Value * 0.1f, q = rotation.Value;
            float4 d = new float4(a.x * q.w + a.y * q.z - a.z * q.y,
                a.y * q.w + a.z * q.x - a.x * q.z,
                a.z * q.w + a.x * q.y - a.y * q.x,
                -((a.x * q.x + a.y * q.y) + a.z * q.z));
            q += d * (0.5f * MathContract.Dt);
            float norm = ((q.x * q.x + q.y * q.y) + q.z * q.z) + q.w * q.w;
            rotation.Value = q * (1f / math.sqrt(norm));
        }
    }

    [BurstCompile(CompileSynchronously = true, FloatMode = FloatMode.Strict, FloatPrecision = FloatPrecision.Standard)]
    public partial struct ParticleJob : IJobEntity
    {
        void Execute(ref Position position, ref Velocity velocity)
        {
            float4 v = (velocity.Value + new float4(0, -9.81f, 0, 0) * MathContract.Dt) * 0.999f;
            float4 p = position.Value + v * MathContract.Dt;
            if (p.y < 0) { p.y = -p.y; v.y = math.abs(v.y) * 0.8f; }
            position.Value = p; velocity.Value = v;
        }
    }

    [BurstCompile(CompileSynchronously = true)]
    public partial struct SnapshotJob : IJobEntity
    {
        [NativeDisableParallelForRestriction] public NativeArray<float4> P;
        [NativeDisableParallelForRestriction] public NativeArray<float4> V;
        void Execute(in StableId id, in Position p, in Velocity v) { P[id.Value] = p.Value; V[id.Value] = v.Value; }
    }

    [BurstCompile(CompileSynchronously = true, FloatMode = FloatMode.Strict)]
    public struct GridJob : IJob
    {
        [ReadOnly] public NativeArray<float4> P;
        public NativeArray<int> Offsets, Ids, Cursor;
        public void Execute()
        {
            for (int i = 0; i <= 4096; i++) Offsets[i] = 0;
            for (int i = 0; i < P.Length; i++) Offsets[MathContract.Key(MathContract.Cell(P[i])) + 1]++;
            for (int c = 0; c < 4096; c++) Offsets[c + 1] += Offsets[c];
            for (int c = 0; c < 4096; c++) Cursor[c] = Offsets[c];
            for (int i = 0; i < P.Length; i++) { int key = MathContract.Key(MathContract.Cell(P[i])); Ids[Cursor[key]] = i; Cursor[key]++; }
        }
    }

    [BurstCompile(CompileSynchronously = true, FloatMode = FloatMode.Strict, FloatPrecision = FloatPrecision.Standard)]
    public partial struct BoidJob : IJobEntity
    {
        [ReadOnly] public NativeArray<float4> P, V;
        [ReadOnly] public NativeArray<int> Offsets, Ids;
        void Execute(in StableId stableId, ref Position position, ref Velocity velocity)
        {
            int id = stableId.Value; float4 p = P[id], v = V[id];
            float3 separation = 0, alignment = 0, cohesion = 0;
            int count = 0; int3 c = MathContract.Cell(p);
            for (int z = math.max(c.z - 1, 0); z <= math.min(c.z + 1, 15); z++)
            for (int y = math.max(c.y - 1, 0); y <= math.min(c.y + 1, 15); y++)
            for (int x = math.max(c.x - 1, 0); x <= math.min(c.x + 1, 15); x++)
            {
                int key = x + 16 * (y + 16 * z);
                for (int index = Offsets[key]; index < Offsets[key + 1]; index++)
                {
                    int j = Ids[index]; float3 d = (p - P[j]).xyz;
                    float d2 = (d.x * d.x + d.y * d.y) + d.z * d.z;
                    if (j != id && d2 < 16f && d2 > 0f)
                    {
                        separation += d / math.max(d2, 0.01f);
                        alignment += V[j].xyz; cohesion += P[j].xyz; count++;
                    }
                }
            }
            if (count > 0)
            {
                float inv = 1f / count;
                float3 acceleration = separation * 0.02f + (alignment * inv - v.xyz) * 0.05f + (cohesion * inv - p.xyz) * 0.01f;
                v += new float4(acceleration * MathContract.Dt, 0);
            }
            float speed2 = (v.x * v.x + v.y * v.y) + v.z * v.z;
            if (speed2 > 16f) v *= 4f / math.sqrt(speed2);
            float4 next = p + v * MathContract.Dt;
            for (int axis = 0; axis < 3; axis++) { if (next[axis] >= 32f) next[axis] -= 64f; if (next[axis] < -32f) next[axis] += 64f; }
            position.Value = next; velocity.Value = v;
        }
    }

    [BurstCompile(CompileSynchronously = true, FloatMode = FloatMode.Strict)]
    public struct ChurnJob : IJobParallelFor
    {
        public uint Frame;
        public EntityArchetype Archetype;
        public EntityCommandBuffer.ParallelWriter Commands;
        [NativeDisableParallelForRestriction] public NativeArray<Entity> Slots;
        public void Execute(int i)
        {
            int slot = (int)(Frame % 10) * 10000 + i;
            uint id = 100000u + Frame * 10000u + (uint)i;
            MathContract.Initial(id, out var p, out var v, out var q);
            Commands.DestroyEntity(i, Slots[slot]);
            Entity entity = Commands.CreateEntity(i, Archetype);
            Commands.SetComponent(i, entity, new StableId { Value = (int)id });
            Commands.SetComponent(i, entity, new Position { Value = p });
            Commands.SetComponent(i, entity, new Velocity { Value = v });
            Commands.SetComponent(i, entity, new Rotation { Value = q });
            // ECB deferred entities cannot be retained across playback. Rebuild Slots afterwards.
        }
    }

    [BurstCompile(CompileSynchronously = true)]
    public partial struct CollectSlotsJob : IJobEntity
    {
        [NativeDisableParallelForRestriction] public NativeArray<Entity> Slots;
        void Execute(Entity entity, in StableId id) { Slots[id.Value % Slots.Length] = entity; }
    }
}
