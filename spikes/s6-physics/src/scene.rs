//! Scene geometry shared by both engines, so every scenario feeds identical inputs.

use crate::common::{Lcg, Quat, Vec3};
use crate::engine::{BodyId, DynamicDesc, Engine, ShapeDesc, VehicleDesc};

/// Large static ground slab with its top face at y = 0.
pub fn ground<E: Engine>(e: &mut E, half: f32) -> BodyId {
    e.add_static_box(
        Vec3::new(0.0, -0.5, 0.0),
        Quat::IDENTITY,
        Vec3::new(half, 0.5, half),
    )
}

/// Paraboloid bowl as a static triangle mesh: y = depth * (r / radius)^2, centred at `center`.
/// Triangles wind counter-clockwise seen from above, so normals face into the bowl.
pub fn bowl<E: Engine>(e: &mut E, center: Vec3, radius: f32, depth: f32) -> BodyId {
    let (rings, segments) = (24u32, 48u32);
    let mut verts = vec![center];
    for ring in 1..=rings {
        let r = radius * ring as f32 / rings as f32;
        let y = depth * (r / radius).powi(2);
        for s in 0..segments {
            let a = std::f32::consts::TAU * s as f32 / segments as f32;
            verts.push(center + Vec3::new(r * a.cos(), y, r * a.sin()));
        }
    }
    let idx = |ring: u32, s: u32| 1 + (ring - 1) * segments + (s % segments);
    let mut tris = Vec::new();
    for s in 0..segments {
        tris.push([0, idx(1, s + 1), idx(1, s)]);
    }
    for ring in 1..rings {
        for s in 0..segments {
            let (a, b) = (idx(ring, s), idx(ring, s + 1));
            let (c, d) = (idx(ring + 1, s), idx(ring + 1, s + 1));
            tris.push([a, b, d]);
            tris.push([a, d, c]);
        }
    }
    e.add_static_mesh(&verts, &tris)
}

/// `count` dynamic bodies on a grid above `center`, alternating 0.5 m boxes and spheres,
/// with deterministic pseudo-random orientations and small offsets.
pub fn body_grid(
    count: usize,
    center: Vec3,
    per_row: usize,
    spacing: f32,
    seed: u64,
) -> Vec<DynamicDesc> {
    let mut rng = Lcg::new(seed);
    let half_span = (per_row as f32 - 1.0) * spacing * 0.5;
    (0..count)
        .map(|i| {
            let layer = i / (per_row * per_row);
            let in_layer = i % (per_row * per_row);
            let (gx, gz) = (in_layer % per_row, in_layer / per_row);
            let jitter = Vec3::new(rng.range(-0.05, 0.05), 0.0, rng.range(-0.05, 0.05));
            let axis = Vec3::new(
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0) + 2.0,
            )
            .normalize();
            let rotation = Quat::from_axis_angle(axis, rng.range(0.0, std::f32::consts::PI));
            let position = center
                + Vec3::new(
                    gx as f32 * spacing - half_span,
                    layer as f32 * spacing,
                    gz as f32 * spacing - half_span,
                )
                + jitter;
            let shape = if i % 2 == 0 {
                ShapeDesc::Box {
                    half: Vec3::splat(0.25),
                }
            } else {
                ShapeDesc::Sphere { radius: 0.25 }
            };
            DynamicDesc {
                shape,
                position,
                rotation,
                friction: 0.5,
            }
        })
        .collect()
}

/// Square pyramid of unit boxes with a `base` x `base` bottom layer, resting on y = 0.
pub fn pyramid(base: usize, center: Vec3) -> Vec<DynamicDesc> {
    let mut out = Vec::new();
    for layer in 0..base {
        let n = base - layer;
        let offset = (n as f32 - 1.0) * 0.5;
        for i in 0..n {
            for k in 0..n {
                out.push(DynamicDesc {
                    shape: ShapeDesc::Box {
                        half: Vec3::splat(0.5),
                    },
                    position: center
                        + Vec3::new(i as f32 - offset, 0.5 + layer as f32, k as f32 - offset),
                    rotation: Quat::IDENTITY,
                    friction: 0.6,
                });
            }
        }
    }
    out
}

/// Single column of unit boxes resting on y = 0.
pub fn tower(height: usize, center: Vec3) -> Vec<DynamicDesc> {
    (0..height)
        .map(|i| DynamicDesc {
            shape: ShapeDesc::Box {
                half: Vec3::splat(0.5),
            },
            position: center + Vec3::new(0.0, 0.5 + i as f32, 0.0),
            rotation: Quat::IDENTITY,
            friction: 0.6,
        })
        .collect()
}

/// A 1500 kg car, 1.8 x 0.4 x 4 m chassis, 0.35 m wheels, 1.5 Hz / 0.5 damping suspension.
pub fn car(position: Vec3) -> VehicleDesc {
    let (hx, hy, hz) = (0.9, 0.2, 2.0);
    VehicleDesc {
        position,
        half_extents: Vec3::new(hx, hy, hz),
        mass: 1500.0,
        wheel_radius: 0.35,
        wheel_width: 0.2,
        wheel_offsets: [
            Vec3::new(hx, -0.9 * hy, hz - 0.6),
            Vec3::new(-hx, -0.9 * hy, hz - 0.6),
            Vec3::new(hx, -0.9 * hy, -hz + 0.6),
            Vec3::new(-hx, -0.9 * hy, -hz + 0.6),
        ],
        suspension_rest: 0.5,
        suspension_travel: 0.3,
        suspension_frequency: 1.5,
        suspension_damping_ratio: 0.5,
        max_steer_deg: 30.0,
    }
}

/// A ramp whose top surface rises along +Z from (x, 0, z0) at `angle_deg`, `length` long,
/// followed by a flat landing `landing` metres long. Returns the landing height.
pub fn ramp<E: Engine>(
    e: &mut E,
    x: f32,
    z0: f32,
    angle_deg: f32,
    length: f32,
    landing: f32,
) -> f32 {
    let t = angle_deg.to_radians();
    let thickness = 0.25;
    let rot = Quat::from_rotation_x(-t);
    let dir = Vec3::new(0.0, t.sin(), t.cos());
    let normal = Vec3::new(0.0, t.cos(), -t.sin());
    let top_mid = Vec3::new(x, 0.0, z0) + dir * (length * 0.5);
    e.add_static_box(
        top_mid - normal * thickness,
        rot,
        Vec3::new(1.5, thickness, length * 0.5),
    );
    let height = length * t.sin();
    let z_top = z0 + length * t.cos();
    e.add_static_box(
        Vec3::new(x, height * 0.5, z_top + landing * 0.5),
        Quat::IDENTITY,
        Vec3::new(1.5, height * 0.5, landing * 0.5),
    );
    height
}

/// Stairs rising along +Z from z0: `steps` steps of `rise` x `tread`, then a 6 m landing.
/// Returns the landing height.
pub fn stairs<E: Engine>(e: &mut E, x: f32, z0: f32, steps: usize, rise: f32, tread: f32) -> f32 {
    for i in 0..steps {
        let h = (i + 1) as f32 * rise;
        e.add_static_box(
            Vec3::new(x, h * 0.5, z0 + tread * (i as f32 + 0.5)),
            Quat::IDENTITY,
            Vec3::new(1.5, h * 0.5, tread * 0.5),
        );
    }
    let top = steps as f32 * rise;
    let z_land = z0 + tread * steps as f32;
    e.add_static_box(
        Vec3::new(x, top * 0.5, z_land + 3.0),
        Quat::IDENTITY,
        Vec3::new(1.5, top * 0.5, 3.0),
    );
    top
}
