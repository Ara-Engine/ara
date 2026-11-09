//! One hot-Debug candidate: emit target-native ISA math and transpose inside Run.
//! These Rust helpers assemble source before compilation; they add no gameplay calls.

/// Compiled ISA choice; the emitted capability guard may select scalar execution.
pub fn backend() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64-advsimd"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64-sse"
    } else {
        "scalar-fallback"
    }
}

/// Selects one strict kernel with the same span signature and scalar-tail contract.
pub fn query(workload: &str) -> String {
    let code = match workload {
        "w1" => transform(),
        "w3" => particle(),
        _ => unreachable!("validated workload"),
    };
    native_math(code)
}

// Only setup-time source substitution: emitted Run calls concrete ISA intrinsics,
// never these Rust helpers, generic vector math wrappers or managed math helpers.
fn native_math(mut code: String) -> String {
    let arm = cfg!(target_arch = "aarch64");
    let replacements = if arm {
        [
            ("Vector128.Add(", "AdvSimd.Add("),
            ("Vector128.Subtract(", "AdvSimd.Subtract("),
            ("Vector128.Multiply(", "AdvSimd.Multiply("),
            ("Vector128.Divide(", "AdvSimd.Arm64.Divide("),
            ("Vector128.Sqrt(", "AdvSimd.Arm64.Sqrt("),
            ("Vector128.LessThan(", "AdvSimd.CompareLessThan("),
        ]
    } else {
        [
            ("Vector128.Add(", "Sse.Add("),
            ("Vector128.Subtract(", "Sse.Subtract("),
            ("Vector128.Multiply(", "Sse.Multiply("),
            ("Vector128.Divide(", "Sse.Divide("),
            ("Vector128.Sqrt(", "Sse.Sqrt("),
            ("Vector128.LessThan(", "Sse.CompareLessThan("),
        ]
    };
    let substitutions = if arm {
        [
            ("@direct_supported@", "AdvSimd.Arm64.IsSupported"),
            ("@direct_sign_mask@", ""),
            ("@direct_negative_dot@", "AdvSimd.Negate(dot)"),
            ("@direct_negative_y@", "AdvSimd.Negate(py)"),
            ("@direct_absolute_vy@", "AdvSimd.Abs(vy)"),
            (
                "@direct_select_y@",
                "AdvSimd.BitwiseSelect(bounce, negativeY, py)",
            ),
            (
                "@direct_select_vy@",
                "AdvSimd.BitwiseSelect(bounce, reflectedVy, vy)",
            ),
        ]
    } else {
        [
            ("@direct_supported@", "Sse.IsSupported"),
            (
                "@direct_sign_mask@",
                "var signMask = Vector128.Create(-0.0f);",
            ),
            ("@direct_negative_dot@", "Sse.Xor(dot, signMask)"),
            ("@direct_negative_y@", "Sse.Xor(py, signMask)"),
            ("@direct_absolute_vy@", "Sse.AndNot(signMask, vy)"),
            (
                "@direct_select_y@",
                "Sse.Or(Sse.And(bounce, negativeY), Sse.AndNot(bounce, py))",
            ),
            (
                "@direct_select_vy@",
                "Sse.Or(Sse.And(bounce, reflectedVy), Sse.AndNot(bounce, vy))",
            ),
        ]
    };
    for (from, to) in replacements.into_iter().chain(substitutions) {
        code = code.replace(from, to);
    }
    assert!(
        !code.contains("@direct_"),
        "all native source markers expanded"
    );
    code
}

/// Emits a bit-preserving transpose at the call site, including a portable fallback.
fn transpose(input: [&str; 4], output: &str) -> String {
    let [a, b, c, d] = input;
    format!(
        r#"
            Vector128<float> {output}x, {output}y, {output}z, {output}w;
            {{
                var a = {a}; var b = {b}; var c = {c}; var d = {d};
                if (AdvSimd.Arm64.IsSupported)
                {{
                    var ac0 = AdvSimd.Arm64.ZipLow(a, c); var ac1 = AdvSimd.Arm64.ZipHigh(a, c);
                    var bd0 = AdvSimd.Arm64.ZipLow(b, d); var bd1 = AdvSimd.Arm64.ZipHigh(b, d);
                    {output}x = AdvSimd.Arm64.ZipLow(ac0, bd0);
                    {output}y = AdvSimd.Arm64.ZipHigh(ac0, bd0);
                    {output}z = AdvSimd.Arm64.ZipLow(ac1, bd1);
                    {output}w = AdvSimd.Arm64.ZipHigh(ac1, bd1);
                }}
                else if (Sse.IsSupported)
                {{
                    var ab0 = Sse.UnpackLow(a, b); var ab1 = Sse.UnpackHigh(a, b);
                    var cd0 = Sse.UnpackLow(c, d); var cd1 = Sse.UnpackHigh(c, d);
                    {output}x = Sse.MoveLowToHigh(ab0, cd0); {output}y = Sse.MoveHighToLow(cd0, ab0);
                    {output}z = Sse.MoveLowToHigh(ab1, cd1); {output}w = Sse.MoveHighToLow(cd1, ab1);
                }}
                else
                {{
                    {output}x = Vector128.Create(a.GetElement(0), b.GetElement(0), c.GetElement(0), d.GetElement(0));
                    {output}y = Vector128.Create(a.GetElement(1), b.GetElement(1), c.GetElement(1), d.GetElement(1));
                    {output}z = Vector128.Create(a.GetElement(2), b.GetElement(2), c.GetElement(2), d.GetElement(2));
                    {output}w = Vector128.Create(a.GetElement(3), b.GetElement(3), c.GetElement(3), d.GetElement(3));
                }}
            }}
"#
    )
}

fn load_transposed(column: &str, output: &str) -> String {
    let rows = std::array::from_fn::<_, 4, _>(|row| load(column, row));
    transpose(rows.each_ref().map(String::as_str), output)
}

fn load(column: &str, row: usize) -> String {
    format!(
        "Unsafe.ReadUnaligned<Vector128<float>>(ref Unsafe.Add(ref {column}, ((nint)i + {row}) * 16))"
    )
}

fn store_transposed(column: &str, input: [&str; 4]) -> String {
    let mut code = transpose(input, "row");
    for (row, axis) in ["x", "y", "z", "w"].into_iter().enumerate() {
        code.push_str(&format!(
            "            Unsafe.WriteUnaligned(ref Unsafe.Add(ref {column}, ((nint)i + {row}) * 16), row{axis});\n"
        ));
    }
    code
}

fn transform() -> String {
    let mut code = String::from(
        r#"
    [Query] static void Run(Span<Position> positions, ReadOnlySpan<Velocity> velocities, Span<Rotation> rotations)
    {
        var count = positions.Length;
        if (count != velocities.Length || count != rotations.Length ||
            Unsafe.SizeOf<Position>() != 16 || Unsafe.SizeOf<Velocity>() != 16 || Unsafe.SizeOf<Rotation>() != 16)
            throw new InvalidOperationException("direct kernel requires matching checked float4 columns");
        if (count == 0) return;
        if (!@direct_supported@)
        {
            for (var scalar = 0; scalar < count; scalar++) Transform(ref positions[scalar], in velocities[scalar], ref rotations[scalar]);
            return;
        }
        // Each checked component is a float4 at offset zero, with no SIMD alignment
        // guarantee. Byte refs and explicit unaligned access keep every native load/store
        // valid; i + 3 < count bounds each group and native-size byte offsets avoid overflow.
        ref var p = ref Unsafe.As<Position, byte>(ref System.Runtime.InteropServices.MemoryMarshal.GetReference(positions));
        ref var v = ref Unsafe.As<Velocity, byte>(ref System.Runtime.InteropServices.MemoryMarshal.GetReference(velocities));
        ref var q = ref Unsafe.As<Rotation, byte>(ref System.Runtime.InteropServices.MemoryMarshal.GetReference(rotations));
        var dt = Vector128.Create(Dt); var halfDt = Vector128.Create(0.5f * Dt);
        var tenth = Vector128.Create(0.1f); var one = Vector128.Create(1f);
        @direct_sign_mask@
        var i = 0;
        for (; i <= count - 4; i += 4)
        {
"#,
    );
    for row in 0..4 {
        let p = load("p", row);
        let v = load("v", row);
        code.push_str(&format!("            Unsafe.WriteUnaligned(ref Unsafe.Add(ref p, ((nint)i + {row}) * 16), Vector128.Add({p}, Vector128.Multiply({v}, dt)));\n"));
    }
    // The readonly velocity alias is only loaded; no emitted assignment targets v.
    code.push_str(&load_transposed("v", "a"));
    code.push_str(&load_transposed("q", "q"));
    code.push_str(r#"
            ax = Vector128.Multiply(ax, tenth); ay = Vector128.Multiply(ay, tenth); az = Vector128.Multiply(az, tenth);
            var dx = Vector128.Subtract(Vector128.Add(Vector128.Multiply(ax, qw), Vector128.Multiply(ay, qz)), Vector128.Multiply(az, qy));
            var dy = Vector128.Subtract(Vector128.Add(Vector128.Multiply(ay, qw), Vector128.Multiply(az, qx)), Vector128.Multiply(ax, qz));
            var dz = Vector128.Subtract(Vector128.Add(Vector128.Multiply(az, qw), Vector128.Multiply(ax, qy)), Vector128.Multiply(ay, qx));
            var dot = Vector128.Add(Vector128.Add(Vector128.Multiply(ax, qx), Vector128.Multiply(ay, qy)), Vector128.Multiply(az, qz));
            var dw = @direct_negative_dot@;
            qx = Vector128.Add(qx, Vector128.Multiply(dx, halfDt)); qy = Vector128.Add(qy, Vector128.Multiply(dy, halfDt));
            qz = Vector128.Add(qz, Vector128.Multiply(dz, halfDt)); qw = Vector128.Add(qw, Vector128.Multiply(dw, halfDt));
            var norm = Vector128.Add(Vector128.Add(Vector128.Add(Vector128.Multiply(qx, qx), Vector128.Multiply(qy, qy)), Vector128.Multiply(qz, qz)), Vector128.Multiply(qw, qw));
            var inv = Vector128.Divide(one, Vector128.Sqrt(norm));
            qx = Vector128.Multiply(qx, inv); qy = Vector128.Multiply(qy, inv);
            qz = Vector128.Multiply(qz, inv); qw = Vector128.Multiply(qw, inv);
"#);
    code.push_str(&store_transposed("q", ["qx", "qy", "qz", "qw"]));
    code.push_str(
        r#"
        }
        for (; i < count; i++) Transform(ref positions[i], in velocities[i], ref rotations[i]);
    }
"#,
    );
    code
}

fn particle() -> String {
    let mut code = String::from(
        r#"
    [Query] static void Run(Span<Position> positions, Span<Velocity> velocities)
    {
        var count = positions.Length;
        if (count != velocities.Length || Unsafe.SizeOf<Position>() != 16 || Unsafe.SizeOf<Velocity>() != 16)
            throw new InvalidOperationException("direct kernel requires matching checked float4 columns");
        if (count == 0) return;
        if (!@direct_supported@)
        {
            for (var scalar = 0; scalar < count; scalar++) Particle(ref positions[scalar], ref velocities[scalar]);
            return;
        }
        // Checked float4 layout does not imply SIMD alignment. Access through byte refs
        // and explicit unaligned operations; all offsets use native-size arithmetic.
        ref var p = ref Unsafe.As<Position, byte>(ref System.Runtime.InteropServices.MemoryMarshal.GetReference(positions));
        ref var v = ref Unsafe.As<Velocity, byte>(ref System.Runtime.InteropServices.MemoryMarshal.GetReference(velocities));
        var dt = Vector128.Create(Dt); var zero = Vector128<float>.Zero;
        var gravity = Vector128.Create(-9.81f * Dt); var drag = Vector128.Create(0.999f);
        var restitution = Vector128.Create(0.8f);
        @direct_sign_mask@
        var i = 0;
        for (; i <= count - 4; i += 4)
        {
"#,
    );
    code.push_str(&load_transposed("p", "p"));
    code.push_str(&load_transposed("v", "v"));
    code.push_str(r#"
            vx = Vector128.Multiply(Vector128.Add(vx, zero), drag); vy = Vector128.Multiply(Vector128.Add(vy, gravity), drag);
            vz = Vector128.Multiply(Vector128.Add(vz, zero), drag); vw = Vector128.Multiply(Vector128.Add(vw, zero), drag);
            px = Vector128.Add(px, Vector128.Multiply(vx, dt)); py = Vector128.Add(py, Vector128.Multiply(vy, dt));
            pz = Vector128.Add(pz, Vector128.Multiply(vz, dt)); pw = Vector128.Add(pw, Vector128.Multiply(vw, dt));
            var bounce = Vector128.LessThan(py, zero);
            var negativeY = @direct_negative_y@;
            var reflectedVy = Vector128.Multiply(@direct_absolute_vy@, restitution);
            py = @direct_select_y@;
            vy = @direct_select_vy@;
"#);
    // Separate blocks keep the two output transposes' row locals independent.
    code.push_str("            {\n");
    code.push_str(&store_transposed("p", ["px", "py", "pz", "pw"]));
    code.push_str("            }\n            {\n");
    code.push_str(&store_transposed("v", ["vx", "vy", "vz", "vw"]));
    code.push_str(
        r#"
            }
        }
        for (; i < count; i++) Particle(ref positions[i], ref velocities[i]);
    }
"#,
    );
    code
}
