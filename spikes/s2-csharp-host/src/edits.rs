//! The scripted edit session: 52 saves an agent or designer would plausibly make to `game/`.
//!
//! Each edit is applied to the current text of one file (edits are cumulative within a round).
//! `site` names a `Probe.Stamp(N); // @stamp:<site>` line. If that line is in the edited file the
//! harness rewrites N to this edit's unique version; either way the edit is complete on the first
//! tick in which code stamping the site's current value runs. Edits without a site complete on the
//! first tick after the matching reload report.

/// The outcome an edit should produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// Body-only edit: EnC delta.
    L1,
    /// Structural edit: module swap into a new ALC.
    L3,
    /// Compile error: reported, last good code keeps running.
    Error,
    /// Body edit that makes a system throw: contained, system disabled until the next reload.
    Fault,
    /// Comment/whitespace only: nothing reloads.
    Noop,
}

pub struct Edit {
    pub name: &'static str,
    /// Path relative to the module directory.
    pub file: &'static str,
    pub expect: Expect,
    pub site: Option<&'static str>,
    /// (find, replace) pairs; each `find` must occur exactly once. An empty `find` creates the file.
    /// `{v}` in a replacement becomes the edit's version.
    pub steps: &'static [(&'static str, &'static str)],
}

use Expect::*;

const fn e(
    name: &'static str,
    file: &'static str,
    expect: Expect,
    site: Option<&'static str>,
    steps: &'static [(&'static str, &'static str)],
) -> Edit {
    Edit {
        name,
        file,
        expect,
        site,
        steps,
    }
}

pub const EDITS: &[Edit] = &[
    e(
        "integrate: split step",
        "Movement.cs",
        L1,
        Some("integrate"),
        &[(
            "            x.Position += v.Linear * dt;\n",
            "            Vector3 step = v.Linear * dt;\n            x.Position += step;\n",
        )],
    ),
    e(
        "spin: early-out for zero angular velocity",
        "Movement.cs",
        L1,
        Some("integrate"),
        &[(
            "        Quaternion dq = new Quaternion(w.X, w.Y, w.Z, 0f) * q;",
            "        if (w.LengthSquared() < 1e-12f) return q;\n        Quaternion dq = new Quaternion(w.X, w.Y, w.Z, 0f) * q;",
        )],
    ),
    e(
        "bounds: clamp height",
        "Movement.cs",
        L1,
        Some("bounds"),
        &[(
            "            p.Z = Wrap(p.Z, Tuning.HalfExtent);\n",
            "            p.Z = Wrap(p.Z, Tuning.HalfExtent);\n            if (p.Y > 200f) p.Y = 200f;\n",
        )],
    ),
    e(
        "wrap: expression body -> block body",
        "Movement.cs",
        L1,
        Some("bounds"),
        &[(
            "    static float Wrap(float v, float half) => v > half ? v - 2f * half : v < -half ? v + 2f * half : v;",
            "    static float Wrap(float v, float half)\n    {\n        if (v > half) return v - 2f * half;\n        if (v < -half) return v + 2f * half;\n        return v;\n    }",
        )],
    ),
    e(
        "gravity: ground friction on bounce",
        "Forces.cs",
        L1,
        Some("gravity"),
        &[(
            "            if (tr[i].Position.Y < 0f && v.Linear.Y < 0f) v.Linear.Y = -v.Linear.Y * Tuning.Restitution;",
            "            if (tr[i].Position.Y < 0f && v.Linear.Y < 0f)\n            {\n                v.Linear.Y = -v.Linear.Y * Tuning.Restitution;\n                v.Linear.X *= 0.9f;\n                v.Linear.Z *= 0.9f;\n            }",
        )],
    ),
    e(
        "drag: extra linear damping",
        "Forces.cs",
        L1,
        Some("drag"),
        &[(
            "        float k = MathF.Pow(Tuning.LinearDamping, t.Dt * 60f);",
            "        float k = MathF.Pow(Tuning.LinearDamping, t.Dt * 60f) * 0.9999f;",
        )],
    ),
    e(
        "swirl: wider core",
        "Steering.cs",
        L1,
        Some("swirl"),
        &[("p.Z * p.Z + 1f;", "p.Z * p.Z + 25f;")],
    ),
    e(
        "swirl: lift near the ground",
        "Steering.cs",
        L1,
        Some("swirl"),
        &[(
            "            vel[i].Linear += tangent * (Tuning.SwirlStrength / r2) * dt;\n",
            "            vel[i].Linear += tangent * (Tuning.SwirlStrength / r2) * dt;\n            if (p.Y < 1f) vel[i].Linear.Y += 0.5f * dt;\n",
        )],
    ),
    e(
        "regen: faster when low",
        "Vitals.cs",
        L1,
        Some("regen"),
        &[(
            "            h.Value += h.Regen * t.Dt;",
            "            h.Value += h.Regen * t.Dt * (h.Value < h.Max * 0.25f ? 2f : 1f);",
        )],
    ),
    e(
        "regen: clamp reset value",
        "Vitals.cs",
        L1,
        Some("regen"),
        &[(
            "if (h.Value >= h.Max) h.Value = h.Max * Tuning.RegenResetFraction;",
            "if (h.Value >= h.Max) h.Value = MathF.Max(1f, h.Max * Tuning.RegenResetFraction);",
        )],
    ),
    e(
        "pulse: finer phase spread",
        "Visuals.cs",
        L1,
        Some("pulse"),
        &[("(ids[i].Bits & 63) * 0.1f", "(ids[i].Bits & 127) * 0.05f")],
    ),
    e(
        "pulse: halve amplitude",
        "Visuals.cs",
        L1,
        Some("pulse"),
        &[(
            "1f + Tuning.PulseAmount * MathF.Sin(",
            "1f + Tuning.PulseAmount * 0.5f * MathF.Sin(",
        )],
    ),
    e(
        "drag: off-by-one loop (throws)",
        "Forces.cs",
        Fault,
        Some("drag"),
        &[(
            "        for (int i = 0; i < vel.Length; i++)\n        {\n            ref Velocity v = ref vel[i];\n            v.Linear *= k;",
            "        for (int i = 0; i <= vel.Length; i++)\n        {\n            ref Velocity v = ref vel[i];\n            v.Linear *= k;",
        )],
    ),
    e(
        "drag: fix off-by-one",
        "Forces.cs",
        L1,
        Some("drag"),
        &[(
            "        for (int i = 0; i <= vel.Length; i++)\n        {\n            ref Velocity v = ref vel[i];\n            v.Linear *= k;",
            "        for (int i = 0; i < vel.Length; i++)\n        {\n            ref Velocity v = ref vel[i];\n            v.Linear *= k;",
        )],
    ),
    e(
        "gravity: typo (compile error)",
        "Forces.cs",
        Error,
        None,
        &[(
            "v.Linear.Y -= Tuning.Gravity * dt;",
            "v.Linear.Y -= Tuning.Gravty * dt;",
        )],
    ),
    e(
        "gravity: fix typo",
        "Forces.cs",
        L1,
        Some("gravity"),
        &[(
            "v.Linear.Y -= Tuning.Gravty * dt;",
            "v.Linear.Y -= Tuning.Gravity * dt;",
        )],
    ),
    e(
        "movement: add Speed() and cap step",
        "Movement.cs",
        L3,
        Some("integrate"),
        &[
            (
                "    /// <summary>Wraps positions",
                "    /// <summary>Speed of a velocity in m/s.</summary>\n    public static float Speed(in Velocity v) => v.Linear.Length();\n\n    /// <summary>Wraps positions",
            ),
            (
                "            Vector3 step = v.Linear * dt;",
                "            Vector3 step = Speed(v) > 50f ? Vector3.Normalize(v.Linear) * (50f * dt) : v.Linear * dt;",
            ),
        ],
    ),
    e(
        "speed: sqrt of length squared",
        "Movement.cs",
        L1,
        Some("integrate"),
        &[(
            "public static float Speed(in Velocity v) => v.Linear.Length();",
            "public static float Speed(in Velocity v) => MathF.Sqrt(v.Linear.LengthSquared());",
        )],
    ),
    e(
        "spin: signature takes `in` params",
        "Movement.cs",
        L3,
        Some("integrate"),
        &[
            (
                "static Quaternion Spin(Quaternion q, Vector3 w, float dt)",
                "static Quaternion Spin(in Quaternion q, in Vector3 w, float dt)",
            ),
            (
                "x.Rotation = Spin(x.Rotation, v.Angular, dt);",
                "x.Rotation = Spin(in x.Rotation, in v.Angular, dt);",
            ),
        ],
    ),
    e(
        "integrate: slower spin",
        "Movement.cs",
        L1,
        Some("integrate"),
        &[(
            "Spin(in x.Rotation, in v.Angular, dt);",
            "Spin(in x.Rotation, in v.Angular, dt * 0.9f);",
        )],
    ),
    e(
        "bounds: add generic Clamp<T>",
        "Movement.cs",
        L3,
        Some("bounds"),
        &[
            (
                "    /// <summary>Wraps positions",
                "    /// <summary>Clamps any number type.</summary>\n    static T Clamp<T>(T v, T lo, T hi) where T : INumber<T> => T.Max(lo, T.Min(hi, v));\n\n    /// <summary>Wraps positions",
            ),
            (
                "            if (p.Y > 200f) p.Y = 200f;",
                "            p.Y = Clamp(p.Y, -50f, 200f);",
            ),
        ],
    ),
    e(
        "clamp: generic body edit",
        "Movement.cs",
        L1,
        Some("bounds"),
        &[(
            "=> T.Max(lo, T.Min(hi, v));",
            "=> v < lo ? lo : v > hi ? hi : v;",
        )],
    ),
    e(
        "tuning: add GroundFriction const",
        "Tuning.cs",
        L3,
        None,
        &[(
            "    /// <summary>Strength of the swirl",
            "    /// <summary>Horizontal speed kept on a ground bounce.</summary>\n    public const float GroundFriction = 0.9f;\n    /// <summary>Strength of the swirl",
        )],
    ),
    e(
        "gravity: use GroundFriction",
        "Forces.cs",
        L1,
        Some("gravity"),
        &[
            (
                "v.Linear.X *= 0.9f;",
                "v.Linear.X *= Tuning.GroundFriction;",
            ),
            (
                "v.Linear.Z *= 0.9f;",
                "v.Linear.Z *= Tuning.GroundFriction;",
            ),
        ],
    ),
    e(
        "tuning: bouncier restitution",
        "Tuning.cs",
        L3,
        None,
        &[(
            "public const float Restitution = 0.5f;",
            "public const float Restitution = 0.6f;",
        )],
    ),
    e(
        "swirl: extract Tangent()",
        "Steering.cs",
        L3,
        Some("swirl"),
        &[
            (
                "            Vector3 tangent = new(-p.Z, 0f, p.X);",
                "            Vector3 tangent = Tangent(p);",
            ),
            (
                "    /// <summary>Pushes entities around",
                "    static Vector3 Tangent(Vector3 p) => new(-p.Z, 0f, p.X);\n\n    /// <summary>Pushes entities around",
            ),
        ],
    ),
    e(
        "tangent: tilt with height",
        "Steering.cs",
        L1,
        Some("swirl"),
        &[("=> new(-p.Z, 0f, p.X);", "=> new(-p.Z, 0.05f * p.Y, p.X);")],
    ),
    e(
        "swirl: add SwirlParams record struct",
        "Steering.cs",
        L3,
        Some("swirl"),
        &[
            (
                "/// <summary>Steering forces that keep the crowd moving.</summary>\npublic static class Steering",
                "/// <summary>Swirl tuning bundle.</summary>\npublic readonly record struct SwirlParams(float Strength, float Core);\n\n/// <summary>Steering forces that keep the crowd moving.</summary>\npublic static class Steering",
            ),
            (
                "        float dt = t.Dt;\n        Span<Velocity> vel",
                "        float dt = t.Dt;\n        var prm = new SwirlParams(Tuning.SwirlStrength, 25f);\n        Span<Velocity> vel",
            ),
            ("p.Z * p.Z + 25f;", "p.Z * p.Z + prm.Core;"),
            ("(Tuning.SwirlStrength / r2)", "(prm.Strength / r2)"),
        ],
    ),
    e(
        "swirl: retune params",
        "Steering.cs",
        L1,
        Some("swirl"),
        &[(
            "new SwirlParams(Tuning.SwirlStrength, 25f)",
            "new SwirlParams(Tuning.SwirlStrength * 1.2f, 16f)",
        )],
    ),
    e(
        "regen: uninitialised history array (throws)",
        "Vitals.cs",
        Fault,
        Some("regen"),
        &[
            (
                "        Span<Health> hp = c.Health;\n",
                "        Span<Health> hp = c.Health;\n        Health[]? history = null;\n",
            ),
            (
                "            ref Health h = ref hp[i];\n",
                "            ref Health h = ref hp[i];\n            history![i & 7] = h;\n",
            ),
        ],
    ),
    e(
        "regen: remove history",
        "Vitals.cs",
        L1,
        Some("regen"),
        &[
            (
                "        Span<Health> hp = c.Health;\n        Health[]? history = null;\n",
                "        Span<Health> hp = c.Health;\n",
            ),
            (
                "            ref Health h = ref hp[i];\n            history![i & 7] = h;\n",
                "            ref Health h = ref hp[i];\n",
            ),
        ],
    ),
    e(
        "regen: extract RegenRate()",
        "Vitals.cs",
        L3,
        Some("regen"),
        &[
            (
                "            h.Value += h.Regen * t.Dt * (h.Value < h.Max * 0.25f ? 2f : 1f);",
                "            h.Value += h.Regen * t.Dt * RegenRate(h);",
            ),
            (
                "    /// <summary>Regenerates health;",
                "    static float RegenRate(in Health h) => h.Value < h.Max * 0.25f ? 2f : 1f;\n\n    /// <summary>Regenerates health;",
            ),
        ],
    ),
    e(
        "regenrate: stronger boost",
        "Vitals.cs",
        L1,
        Some("regen"),
        &[(
            "=> h.Value < h.Max * 0.25f ? 2f : 1f;",
            "=> h.Value < h.Max * 0.25f ? 2.5f : 1f;",
        )],
    ),
    e(
        "visuals: add Wobble system (unregistered)",
        "Visuals.cs",
        L3,
        None,
        &[
            ("using Ara;", "using System.Numerics;\nusing Ara;"),
            (
                "    /// <summary>Pulses each entity's scale",
                "    /// <summary>Tilts entities slightly over time.</summary>\n    public static void Wobble(PlaceChunk c, in Tick t)\n    {\n        Probe.Stamp({v}); // @stamp:wobble\n        Span<Transform> tr = c.Transform;\n        float a = 0.01f * MathF.Sin(t.Elapsed);\n        for (int i = 0; i < tr.Length; i++)\n            tr[i].Rotation = Quaternion.Normalize(tr[i].Rotation + new Quaternion(a, 0f, 0f, 0f));\n    }\n\n    /// <summary>Pulses each entity's scale",
            ),
        ],
    ),
    e(
        "register wobble",
        "ModuleEntry.cs",
        L1,
        Some("wobble"),
        &[(
            "        r.Add(\"pulse\", Visuals.Pulse);\n",
            "        r.Add(\"pulse\", Visuals.Pulse);\n        r.Add(\"wobble\", Visuals.Wobble);\n",
        )],
    ),
    e(
        "wobble: gentler and faster",
        "Visuals.cs",
        L1,
        Some("wobble"),
        &[(
            "float a = 0.01f * MathF.Sin(t.Elapsed);",
            "float a = 0.004f * MathF.Sin(t.Elapsed * 2f);",
        )],
    ),
    e(
        "new file: Spawner.cs (unregistered)",
        "Spawner.cs",
        L3,
        None,
        &[(
            "",
            "using Ara;\n\nnamespace Game;\n\n/// <summary>Population bookkeeping: counts low-health entities each tick.</summary>\npublic static class Spawner\n{\n    /// <summary>Counts entities whose health is low (diagnostic system).</summary>\n    public static void Heartbeat(VitalsChunk c, in Tick t)\n    {\n        Probe.Stamp({v}); // @stamp:heartbeat\n        int low = CountWhere(c.Health, static h => h.Value < 10f);\n        if (low < 0) Log.Warn(\"impossible\");\n    }\n\n    static int CountWhere(ReadOnlySpan<Health> xs, Func<Health, bool> pred)\n    {\n        int n = 0;\n        foreach (ref readonly Health h in xs) if (pred(h)) n++;\n        return n;\n    }\n}\n",
        )],
    ),
    e(
        "register heartbeat",
        "ModuleEntry.cs",
        L1,
        Some("heartbeat"),
        &[(
            "        r.Add(\"wobble\", Visuals.Wobble);\n",
            "        r.Add(\"wobble\", Visuals.Wobble);\n        r.Add(\"heartbeat\", Spawner.Heartbeat);\n",
        )],
    ),
    // Escalates (closure rule): applied as an EnC delta without a syntax map, this crashed the process.
    e(
        "heartbeat: lambda body edit",
        "Spawner.cs",
        L3,
        Some("heartbeat"),
        &[("static h => h.Value < 10f", "static h => h.Value < 20f")],
    ),
    e(
        "register: reorder regen after pulse",
        "ModuleEntry.cs",
        L1,
        None,
        &[(
            "        r.Add(\"regen\", Vitals.Regen);\n        r.Add(\"pulse\", Visuals.Pulse);\n",
            "        r.Add(\"pulse\", Visuals.Pulse);\n        r.Add(\"regen\", Vitals.Regen);\n",
        )],
    ),
    e(
        "tuning: weaker swirl",
        "Tuning.cs",
        L3,
        None,
        &[(
            "public const float SwirlStrength = 40f;",
            "public const float SwirlStrength = 30f;",
        )],
    ),
    e(
        "regen: lower reset fraction",
        "Vitals.cs",
        L1,
        Some("regen"),
        &[(
            "h.Max * Tuning.RegenResetFraction)",
            "h.Max * Tuning.RegenResetFraction * 0.9f)",
        )],
    ),
    e(
        "integrate: NaN guard",
        "Movement.cs",
        L1,
        Some("integrate"),
        &[(
            "            x.Position += step;\n",
            "            x.Position += step;\n            if (!float.IsFinite(x.Position.Y)) x.Position.Y = 0f;\n",
        )],
    ),
    e(
        "gravity: extract Grounded()",
        "Forces.cs",
        L3,
        Some("gravity"),
        &[
            (
                "if (tr[i].Position.Y < 0f && v.Linear.Y < 0f)",
                "if (Grounded(tr[i], v))",
            ),
            (
                "    /// <summary>Applies gravity",
                "    static bool Grounded(in Transform t, in Velocity v) => t.Position.Y < 0f && v.Linear.Y < 0f;\n\n    /// <summary>Applies gravity",
            ),
        ],
    ),
    e(
        "grounded: contact epsilon",
        "Forces.cs",
        L1,
        Some("gravity"),
        &[(
            "=> t.Position.Y < 0f && v.Linear.Y < 0f;",
            "=> t.Position.Y < 0.01f && v.Linear.Y < 0f;",
        )],
    ),
    e(
        "drag: extra angular damping",
        "Forces.cs",
        L1,
        Some("drag"),
        &[(
            "float ka = MathF.Pow(Tuning.AngularDamping, t.Dt * 60f);",
            "float ka = MathF.Pow(Tuning.AngularDamping, t.Dt * 60f) * 0.999f;",
        )],
    ),
    e(
        "comment-only change",
        "Movement.cs",
        Noop,
        None,
        &[(
            "keeps entities inside the play area.</summary>",
            "keeps entities inside the play area (wraps X and Z).</summary>",
        )],
    ),
    e(
        "pulse: add TwoPi const, wrap phase",
        "Visuals.cs",
        L3,
        Some("pulse"),
        &[
            (
                "float phase = t.Elapsed * Tuning.PulseRate;",
                "float phase = (t.Elapsed * Tuning.PulseRate) % TwoPi;",
            ),
            (
                "public static class Visuals\n{\n",
                "public static class Visuals\n{\n    const float TwoPi = MathF.PI * 2f;\n\n",
            ),
        ],
    ),
    e(
        "pulse: wrap at 4 pi",
        "Visuals.cs",
        L1,
        Some("pulse"),
        &[("% TwoPi;", "% (TwoPi * 2f);")],
    ),
    e(
        "swirl: stronger lift",
        "Steering.cs",
        L1,
        Some("swirl"),
        &[(
            "if (p.Y < 1f) vel[i].Linear.Y += 0.5f * dt;",
            "if (p.Y < 1f) vel[i].Linear.Y += 0.8f * dt;",
        )],
    ),
    e(
        "vitals: add Drain() system (unregistered)",
        "Vitals.cs",
        L3,
        None,
        &[(
            "    /// <summary>Regenerates health;",
            "    /// <summary>Slowly drains health (poison zones).</summary>\n    public static void Drain(VitalsChunk c, in Tick t)\n    {\n        Probe.Stamp({v}); // @stamp:drain\n        foreach (ref Health h in c.Health) h.Value -= 0.1f * t.Dt;\n    }\n\n    /// <summary>Regenerates health;",
        )],
    ),
    e(
        "register drain",
        "ModuleEntry.cs",
        L1,
        Some("drain"),
        &[(
            "        r.Add(\"heartbeat\", Spawner.Heartbeat);\n",
            "        r.Add(\"heartbeat\", Spawner.Heartbeat);\n        r.Add(\"drain\", Vitals.Drain);\n",
        )],
    ),
];
