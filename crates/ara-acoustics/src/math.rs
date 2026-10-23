//! Minimal `[f32; 3]` vector helpers; acoustics never needs a math crate.

pub(crate) type V3 = [f32; 3];

pub(crate) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(crate) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub(crate) fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}
pub(crate) fn distance(a: V3, b: V3) -> f32 {
    length(sub(a, b))
}
/// Unit vector, or `fallback` for (near-)zero input.
pub(crate) fn normalize_or(a: V3, fallback: V3) -> V3 {
    let l = length(a);
    if l > 1e-6 && l.is_finite() {
        scale(a, 1.0 / l)
    } else {
        fallback
    }
}
pub(crate) fn finite(a: V3) -> bool {
    a.iter().all(|v| v.is_finite())
}
/// Rotates `v` by unit quaternion `q` = [x, y, z, w].
pub(crate) fn rotate(q: [f32; 4], v: V3) -> V3 {
    let u = [q[0], q[1], q[2]];
    let t = scale(cross(u, v), 2.0);
    add(add(v, scale(t, q[3])), cross(u, t))
}
/// Inverse rotation of a unit quaternion.
pub(crate) fn conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}
/// Deterministic, evenly spread unit directions (Fibonacci sphere).
pub(crate) fn fibonacci(i: usize, n: usize) -> V3 {
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    let y = 1.0 - (i as f32 + 0.5) / n.max(1) as f32 * 2.0;
    let r = (1.0 - y * y).max(0.0).sqrt();
    let theta = golden * i as f32;
    [r * theta.cos(), y, r * theta.sin()]
}
