//! World-panel ray to logical-pixel mapping.
/// Intersect a world ray with an orthonormal panel basis, reject back-facing/outside hits, map to logical pixels.
pub fn ray_to_local(
    origin: [f32; 3],
    direction: [f32; 3],
    panel_origin: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    meters: [f32; 2],
    pixels: [f32; 2],
) -> Option<[f32; 2]> {
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let normal = [
        right[1] * up[2] - right[2] * up[1],
        right[2] * up[0] - right[0] * up[2],
        right[0] * up[1] - right[1] * up[0],
    ];
    let denom = dot(direction, normal);
    if denom >= -1e-6 || meters[0] <= 0.0 || meters[1] <= 0.0 {
        return None;
    }
    let delta = [
        panel_origin[0] - origin[0],
        panel_origin[1] - origin[1],
        panel_origin[2] - origin[2],
    ];
    let t = dot(delta, normal) / denom;
    if t < 0.0 {
        return None;
    }
    let relative = [
        origin[0] + direction[0] * t - panel_origin[0],
        origin[1] + direction[1] * t - panel_origin[1],
        origin[2] + direction[2] * t - panel_origin[2],
    ];
    let u = dot(relative, right) / meters[0] + 0.5;
    let v = 0.5 - dot(relative, up) / meters[1];
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        None
    } else {
        Some([u * pixels[0], v * pixels[1]])
    }
}
