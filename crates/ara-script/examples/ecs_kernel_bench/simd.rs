//! Cross-entity SIMD; transpose only changes lane placement, never arithmetic order.
pub const HELPERS: &str = r#"
    [MethodImpl(MethodImplOptions.AggressiveInlining)]
    static void Transpose(Vector128<float> a, Vector128<float> b, Vector128<float> c, Vector128<float> d,
        out Vector128<float> x, out Vector128<float> y, out Vector128<float> z, out Vector128<float> w)
    {
        if (AdvSimd.Arm64.IsSupported)
        {
            var ab0 = AdvSimd.Arm64.ZipLow(a, b); var ab1 = AdvSimd.Arm64.ZipHigh(a, b);
            var cd0 = AdvSimd.Arm64.ZipLow(c, d); var cd1 = AdvSimd.Arm64.ZipHigh(c, d);
            x = AdvSimd.Arm64.ZipLow(ab0.AsUInt64(), cd0.AsUInt64()).AsSingle();
            y = AdvSimd.Arm64.ZipHigh(ab0.AsUInt64(), cd0.AsUInt64()).AsSingle();
            z = AdvSimd.Arm64.ZipLow(ab1.AsUInt64(), cd1.AsUInt64()).AsSingle();
            w = AdvSimd.Arm64.ZipHigh(ab1.AsUInt64(), cd1.AsUInt64()).AsSingle();
        }
        else if (Sse.IsSupported)
        {
            var ab0 = Sse.UnpackLow(a, b); var ab1 = Sse.UnpackHigh(a, b);
            var cd0 = Sse.UnpackLow(c, d); var cd1 = Sse.UnpackHigh(c, d);
            x = Sse.MoveLowToHigh(ab0, cd0); y = Sse.MoveHighToLow(cd0, ab0);
            z = Sse.MoveLowToHigh(ab1, cd1); w = Sse.MoveHighToLow(cd1, ab1);
        }
        else
        {
            x = Vector128.Create(a.GetElement(0), b.GetElement(0), c.GetElement(0), d.GetElement(0));
            y = Vector128.Create(a.GetElement(1), b.GetElement(1), c.GetElement(1), d.GetElement(1));
            z = Vector128.Create(a.GetElement(2), b.GetElement(2), c.GetElement(2), d.GetElement(2));
            w = Vector128.Create(a.GetElement(3), b.GetElement(3), c.GetElement(3), d.GetElement(3));
        }
    }
"#;

pub const W1: &str = r#"
    [Query] static void Run(Span<Position> positions, ReadOnlySpan<Velocity> velocities, Span<Rotation> rotations)
    {
        var halfDt = Vector128.Create(0.5f * Dt);
        var tenth = Vector128.Create(0.1f); var one = Vector128.Create(1f);
        var i = 0;
        for (; i <= positions.Length - 4; i += 4)
        {
            positions[i].Value += velocities[i].Value * Dt;
            positions[i + 1].Value += velocities[i + 1].Value * Dt;
            positions[i + 2].Value += velocities[i + 2].Value * Dt;
            positions[i + 3].Value += velocities[i + 3].Value * Dt;
            Transpose(velocities[i].Value.AsVector128(), velocities[i + 1].Value.AsVector128(),
                velocities[i + 2].Value.AsVector128(), velocities[i + 3].Value.AsVector128(), out var ax, out var ay, out var az, out _);
            ax *= tenth; ay *= tenth; az *= tenth;
            Transpose(rotations[i].Value.AsVector128(), rotations[i + 1].Value.AsVector128(),
                rotations[i + 2].Value.AsVector128(), rotations[i + 3].Value.AsVector128(), out var x, out var y, out var z, out var w);
            var dx = ax * w + ay * z - az * y;
            var dy = ay * w + az * x - ax * z;
            var dz = az * w + ax * y - ay * x;
            var dw = -((ax * x + ay * y) + az * z);
            x += dx * halfDt; y += dy * halfDt; z += dz * halfDt; w += dw * halfDt;
            var norm = ((x * x + y * y) + z * z) + w * w;
            var inv = one / Vector128.Sqrt(norm);
            Transpose(x * inv, y * inv, z * inv, w * inv, out var r0, out var r1, out var r2, out var r3);
            rotations[i].Value = r0.AsVector4(); rotations[i + 1].Value = r1.AsVector4();
            rotations[i + 2].Value = r2.AsVector4(); rotations[i + 3].Value = r3.AsVector4();
        }
        for (; i < positions.Length; i++) Transform(ref positions[i], in velocities[i], ref rotations[i]);
    }
"#;

pub const W3: &str = r#"
    [Query] static void Run(Span<Position> positions, Span<Velocity> velocities)
    {
        var dt = Vector128.Create(Dt); var zero = Vector128<float>.Zero;
        var gravity = Vector128.Create(-9.81f * Dt); var drag = Vector128.Create(0.999f);
        var restitution = Vector128.Create(0.8f);
        var i = 0;
        for (; i <= positions.Length - 4; i += 4)
        {
            Transpose(positions[i].Value.AsVector128(), positions[i + 1].Value.AsVector128(),
                positions[i + 2].Value.AsVector128(), positions[i + 3].Value.AsVector128(), out var px, out var py, out var pz, out var pw);
            Transpose(velocities[i].Value.AsVector128(), velocities[i + 1].Value.AsVector128(),
                velocities[i + 2].Value.AsVector128(), velocities[i + 3].Value.AsVector128(), out var vx, out var vy, out var vz, out var vw);
            vx = (vx + zero) * drag; vy = (vy + gravity) * drag;
            vz = (vz + zero) * drag; vw = (vw + zero) * drag;
            px += vx * dt; py += vy * dt; pz += vz * dt; pw += vw * dt;
            var bounce = Vector128.LessThan(py, zero);
            py = Vector128.ConditionalSelect(bounce, -py, py);
            vy = Vector128.ConditionalSelect(bounce, Vector128.Abs(vy) * restitution, vy);
            Transpose(px, py, pz, pw, out var p0, out var p1, out var p2, out var p3);
            Transpose(vx, vy, vz, vw, out var v0, out var v1, out var v2, out var v3);
            positions[i].Value = p0.AsVector4(); positions[i + 1].Value = p1.AsVector4();
            positions[i + 2].Value = p2.AsVector4(); positions[i + 3].Value = p3.AsVector4();
            velocities[i].Value = v0.AsVector4(); velocities[i + 1].Value = v1.AsVector4();
            velocities[i + 2].Value = v2.AsVector4(); velocities[i + 3].Value = v3.AsVector4();
        }
        for (; i < positions.Length; i++) Particle(ref positions[i], ref velocities[i]);
    }
"#;
