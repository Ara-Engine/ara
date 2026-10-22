//! Filler code that brings the gameplay module up to a realistic size (~4.8k lines). It compiles
//! with the module (so it costs emit time like real code) but is never ticked.

use std::path::Path;

/// Number of filler files written by [`write`].
pub const FILES: usize = 100;

/// Writes `FILES` gameplay-shaped C# files into `dir/Filler`. Returns the number of lines written.
pub fn write(dir: &Path) -> usize {
    let out = dir.join("Filler");
    std::fs::create_dir_all(&out).expect("create Filler dir");
    let mut lines = 0;
    for i in 0..FILES {
        let text = file(i);
        lines += text.lines().count();
        std::fs::write(out.join(format!("Planner{i}.cs")), text).expect("write filler file");
    }
    lines
}

/// One filler file: a value type, a planner class with collections/LINQ/lambdas, and generic helpers
/// that reference the previous file's helper (cross-file binding).
fn file(i: usize) -> String {
    let prev = (i + FILES - 1) % FILES;
    let heavy = i % 7 + 3;
    let lerp = i % 5 + 1;
    let cap = i % 16 + 8;
    format!(
        r#"using System.Numerics;
using Ara;

namespace Game.Filler;

/// <summary>Generated filler #{i}: an inventory item.</summary>
public struct Loot{i}
{{
    public int ItemId;
    public float Weight;
    public byte Rarity;
    public readonly bool Heavy => Weight > {heavy}f;
}}

/// <summary>Generated filler #{i}: a planner with waypoints and a bag.</summary>
public sealed class Planner{i}
{{
    readonly List<Vector3> _waypoints = new();
    readonly Dictionary<int, Loot{i}> _bag = new();
    float _time;
    public int Steps;

    public Vector3 Next(Vector3 from, float dt)
    {{
        _time += dt;
        if (_waypoints.Count == 0) _waypoints.Add(new Vector3(_time, 0f, {i}f));
        var target = _waypoints.OrderBy(w => Vector3.DistanceSquared(w, from)).First();
        Steps++;
        return Vector3.Lerp(from, target, MathF.Min(1f, dt * {lerp}f));
    }}

    public float CarryWeight() => _bag.Values.Where(l => !l.Heavy).Sum(l => l.Weight) + Helper{prev}.Bias({i});

    public bool TryAdd(Loot{i} loot)
    {{
        if (_bag.Count >= {cap}) return false;
        _bag[loot.ItemId] = loot;
        return true;
    }}

    public string Describe(Entity owner) => $"Planner{i} {{owner}} steps={{Steps}} bag={{_bag.Count}} t={{_time:F1}}";
}}

/// <summary>Generated filler #{i}: helpers.</summary>
public static class Helper{i}
{{
    public static float Bias(int seed) => seed switch {{ 0 => 0.5f, 1 => 1f, _ => seed * 0.01f }};
    public static T Max<T>(ReadOnlySpan<T> xs) where T : INumber<T> {{ var m = T.Zero; foreach (var x in xs) m = T.Max(m, x); return m; }}
    public static int Score(in Loot{i} l) => l.Rarity * 10 + (int)l.Weight;
}}
"#
    )
}
