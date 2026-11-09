//! Real gameplay module declarations and scalar kernels, compiled by the public host.
use super::{direct, simd};

const TYPES: &str = r#"
using System.Runtime.Intrinsics;
using System.Runtime.Intrinsics.Arm;
using System.Runtime.Intrinsics.X86;
using System.Runtime.CompilerServices;
namespace EcsKernels;
/// <summary>Authoritative translation float4.</summary>
[Component("BenchPosition")] public partial struct Position {
    /// <summary>Position including the homogeneous w coordinate.</summary>
    public System.Numerics.Vector4 Value;
}
/// <summary>Authoritative velocity float4.</summary>
[Component("BenchVelocity")] public partial struct Velocity {
    /// <summary>Velocity including the unchanged w coordinate.</summary>
    public System.Numerics.Vector4 Value;
}
/// <summary>Authoritative orientation float4.</summary>
[Component("BenchRotation")] public partial struct Rotation {
    /// <summary>Quaternion in x/y/z/w order.</summary>
    public System.Numerics.Vector4 Value;
}
/// <summary>Stable logical identity, independent of native entity handles.</summary>
[Component("BenchStableId")] public partial struct StableId {
    /// <summary>Zero-based logical ID.</summary>
    public uint Value;
}
/// <summary>Strict S8 math over real module ECS storage.</summary>
[Simulation] public static partial class Kernels {
    const float Dt = 1f / 60f;
"#;

const SCALAR: &str = r#"
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    static void Transform(ref Position position, in Velocity velocity, ref Rotation rotation)
    {
        var p = position.Value; var v = velocity.Value; var q = rotation.Value;
        p.X += v.X * Dt; p.Y += v.Y * Dt; p.Z += v.Z * Dt; p.W += v.W * Dt;
        float ax = v.X * 0.1f, ay = v.Y * 0.1f, az = v.Z * 0.1f;
        float dx = ax * q.W + ay * q.Z - az * q.Y;
        float dy = ay * q.W + az * q.X - ax * q.Z;
        float dz = az * q.W + ax * q.Y - ay * q.X;
        float dw = -((ax * q.X + ay * q.Y) + az * q.Z);
        q.X += dx * (0.5f * Dt); q.Y += dy * (0.5f * Dt);
        q.Z += dz * (0.5f * Dt); q.W += dw * (0.5f * Dt);
        float norm = ((q.X * q.X + q.Y * q.Y) + q.Z * q.Z) + q.W * q.W;
        float inv = 1f / MathF.Sqrt(norm);
        q.X *= inv; q.Y *= inv; q.Z *= inv; q.W *= inv;
        position.Value = p; rotation.Value = q;
    }
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    static void Particle(ref Position position, ref Velocity velocity)
    {
        var p = position.Value; var v = velocity.Value;
        v.X = (v.X + 0f * Dt) * 0.999f; v.Y = (v.Y + -9.81f * Dt) * 0.999f;
        v.Z = (v.Z + 0f * Dt) * 0.999f; v.W = (v.W + 0f * Dt) * 0.999f;
        p.X += v.X * Dt; p.Y += v.Y * Dt; p.Z += v.Z * Dt; p.W += v.W * Dt;
        if (p.Y < 0f) { p.Y = -p.Y; v.Y = MathF.Abs(v.Y) * 0.8f; }
        position.Value = p; velocity.Value = v;
    }
"#;

const W1: &str = r#"
    [Query] static void Run(Span<Position> positions, ReadOnlySpan<Velocity> velocities, Span<Rotation> rotations)
    {
        for (var i = 0; i < positions.Length; i++) Transform(ref positions[i], in velocities[i], ref rotations[i]);
    }
"#;
const W3: &str = r#"
    [Query] static void Run(Span<Position> positions, Span<Velocity> velocities)
    {
        for (var i = 0; i < positions.Length; i++) Particle(ref positions[i], ref velocities[i]);
    }
"#;

/// Selects one query, keeping all alternate kernels out of the scheduled workload.
pub fn module(workload: &str, mode: &str) -> String {
    if mode == "direct" {
        let query = direct::query(workload);
        return format!("{TYPES}\n{SCALAR}\n\n{query}\n}}");
    }
    let query = match (workload, mode) {
        ("w1", "span") => W1,
        ("w3", "span") => W3,
        ("w1", "simd") => simd::W1,
        ("w3", "simd") => simd::W3,
        _ => unreachable!("validated workload/mode"),
    };
    let helpers = if mode == "simd" { simd::HELPERS } else { "" };
    format!("{TYPES}\n{SCALAR}\n{helpers}\n{query}\n}}")
}
