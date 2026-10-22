using System.Runtime.Intrinsics;

namespace S8;

/// Identical fixed-step arithmetic to src/model.rs. No frame allocations here.
public static unsafe class Kernels
{
    public const float Dt = 1f / 60f;
    public const int Cells = 4096;

    public static void Initial(uint id, out F4 p, out F4 v, out F4 q)
    {
        static float Unit(uint id, uint salt)
        {
            uint x = id ^ salt;
            x = unchecked((x ^ (x >> 16)) * 0x7feb352du);
            x = unchecked((x ^ (x >> 15)) * 0x846ca68bu);
            return ((x ^ (x >> 16)) & 65535u) / 65536f;
        }
        p = new(Unit(id, 2891336453u) * 64f - 32f, Unit(id, 1181783497u) * 64f - 32f, Unit(id, 425950093u) * 64f - 32f, 1f);
        v = new(Unit(id, 97) * 2f - 1f, Unit(id, 193) * 2f - 1f, Unit(id, 389) * 2f - 1f, 0f);
        q = new(0, 0, 0, 1);
    }

    public static void Transform(Span<F4> positions, ReadOnlySpan<F4> velocities, Span<F4> rotations)
    {
        for (int i = 0; i < positions.Length; i++)
        {
            positions[i] += velocities[i] * Dt;
            F4 a = velocities[i] * 0.1f, q = rotations[i];
            F4 d = new(a.x * q.w + a.y * q.z - a.z * q.y, a.y * q.w + a.z * q.x - a.x * q.z,
                a.z * q.w + a.x * q.y - a.y * q.x, -((a.x * q.x + a.y * q.y) + a.z * q.z));
            q += d * (0.5f * Dt);
            float norm = ((q.x * q.x + q.y * q.y) + q.z * q.z) + q.w * q.w;
            rotations[i] = q * (1f / MathF.Sqrt(norm));
        }
    }

    public static void Particles(Span<F4> positions, Span<F4> velocities)
    {
        for (int i = 0; i < positions.Length; i++)
        {
            F4 v = (velocities[i] + new F4(0, -9.81f, 0, 0) * Dt) * 0.999f;
            F4 p = positions[i] + v * Dt;
            if (p.y < 0) { p.y = -p.y; v.y = MathF.Abs(v.y) * 0.8f; }
            positions[i] = p; velocities[i] = v;
        }
    }

    /// Two adjacent float4 particles per Vector256, including masked plane collisions.
    public static void ParticlesSimd(F4* positions, F4* velocities, int count)
    {
        if (!Vector256.IsHardwareAccelerated) throw new PlatformNotSupportedException("Vector256 required");
        var gravity = Vector256.Create(0f, -9.81f * Dt, 0f, 0f, 0f, -9.81f * Dt, 0f, 0f);
        var yMask = Vector256.Create(0, -1, 0, 0, 0, -1, 0, 0).AsSingle();
        int i = 0;
        for (; i + 1 < count; i += 2)
        {
            var v = (Vector256.Load((float*)(velocities + i)) + gravity) * Vector256.Create(0.999f);
            var p = Vector256.Load((float*)(positions + i)) + v * Vector256.Create(Dt);
            var hit = Vector256.LessThan(p, Vector256<float>.Zero) & yMask;
            p = Vector256.ConditionalSelect(hit, -p, p);
            v = Vector256.ConditionalSelect(hit, Vector256.Abs(v) * Vector256.Create(0.8f), v);
            p.Store((float*)(positions + i)); v.Store((float*)(velocities + i));
        }
        if (i < count) Particles(new Span<F4>(positions + i, 1), new Span<F4>(velocities + i, 1));
    }

    public static int Cell(float x) => Math.Clamp((int)MathF.Floor((x + 32f) * 0.25f), 0, 15);
    public static int Key(F4 p) => Cell(p.x) + 16 * (Cell(p.y) + 16 * Cell(p.z));

    public static void BuildGrid(HostView h)
    {
        new ReadOnlySpan<F4>(h.p, h.count).CopyTo(new Span<F4>(h.old_p, h.count));
        new ReadOnlySpan<F4>(h.v, h.count).CopyTo(new Span<F4>(h.old_v, h.count));
        new Span<int>(h.offsets, Cells + 1).Clear();
        for (int i = 0; i < h.count; i++) h.offsets[Key(h.old_p[i]) + 1]++;
        for (int c = 0; c < Cells; c++) h.offsets[c + 1] += h.offsets[c];
        new ReadOnlySpan<int>(h.offsets, Cells).CopyTo(new Span<int>(h.cursor, Cells));
        for (int i = 0; i < h.count; i++) h.ids[h.cursor[Key(h.old_p[i])]++] = i;
    }

    public static void Boids(HostView h, int start, int end)
    {
        for (int id = start; id < end; id++)
        {
            F4 p = h.old_p[id], v = h.old_v[id], separation = default, alignment = default, cohesion = default;
            int count = 0, cx = Cell(p.x), cy = Cell(p.y), cz = Cell(p.z);
            for (int z = Math.Max(cz - 1, 0); z <= Math.Min(cz + 1, 15); z++)
            for (int y = Math.Max(cy - 1, 0); y <= Math.Min(cy + 1, 15); y++)
            for (int x = Math.Max(cx - 1, 0); x <= Math.Min(cx + 1, 15); x++)
            {
                int key = x + 16 * (y + 16 * z);
                for (int index = h.offsets[key]; index < h.offsets[key + 1]; index++)
                {
                    int j = h.ids[index]; F4 d = p - h.old_p[j];
                    float d2 = (d.x * d.x + d.y * d.y) + d.z * d.z;
                    if (j != id && d2 < 16f && d2 > 0f)
                    {
                        separation += d / Math.Max(d2, 0.01f);
                        alignment += h.old_v[j]; cohesion += h.old_p[j]; count++;
                    }
                }
            }
            if (count > 0)
            {
                float inv = 1f / count;
                F4 acceleration = separation * 0.02f + (alignment * inv - v) * 0.05f + (cohesion * inv - p) * 0.01f;
                acceleration.w = 0;
                v += acceleration * Dt;
            }
            float speed2 = (v.x * v.x + v.y * v.y) + v.z * v.z;
            if (speed2 > 16f) v *= 4f / MathF.Sqrt(speed2);
            F4 next = p + v * Dt;
            static float Wrap(float a) { if (a >= 32f) a -= 64f; if (a < -32f) a += 64f; return a; }
            next.x = Wrap(next.x); next.y = Wrap(next.y); next.z = Wrap(next.z);
            h.p[id] = next; h.v[id] = v;
        }
    }

    public static void Churn(HostView h, int start, int end, uint frame)
    {
        for (int i = start; i < end; i++)
        {
            uint id = 100000u + frame * 10000u + (uint)i;
            Initial(id, out var p, out var v, out var q);
            h.commands[i] = new ChurnCommand { slot = (frame % 10u) * 10000u + (uint)i, id = id, p = p, v = v, q = q };
        }
    }
}
