#![allow(dead_code)]
use ara_physics::*;
use ara_physics_jolt::*;
pub use glam33::{Quat, Vec3};
pub const DT: f32 = 1.0 / 60.0;

pub fn world(threads: u32, max_bodies: u32) -> JoltWorld {
    JoltWorld::new(PhysicsConfig {
        threads,
        max_bodies,
        max_pairs: (max_bodies * 16).max(4096),
        max_contacts: (max_bodies * 16).max(4096),
        event_capacity: 0,
        ..Default::default()
    })
    .expect("valid fixture configuration")
}
pub fn pose(position: Vec3, rotation: Quat) -> PhysicsPose {
    PhysicsPose {
        position: position.to_array(),
        rotation: Rotation(rotation.to_array()),
    }
}
pub fn add_box(
    w: &mut JoltWorld,
    position: Vec3,
    rotation: Quat,
    half: Vec3,
    motion: MotionType,
) -> BodyHandle {
    let shape = w
        .backend_mut()
        .create_shape(Shape::Box {
            half_extents: half.to_array(),
        })
        .expect("valid box");
    add_body(w, shape, position, rotation, motion)
}
pub fn add_body(
    w: &mut JoltWorld,
    shape: ShapeHandle,
    position: Vec3,
    rotation: Quat,
    motion: MotionType,
) -> BodyHandle {
    let mut ids = Vec::new();
    w.backend_mut()
        .create_bodies(
            &[BodyDesc {
                shape,
                user_data: 0,
                pose: pose(position, rotation),
                body: RigidBody {
                    motion,
                    ..Default::default()
                },
                collider: Collider {
                    layer: if motion == MotionType::Static { 0 } else { 1 },
                    ..Default::default()
                },
                velocity: Velocity::default(),
            }],
            &mut ids,
        )
        .expect("valid fixture body");
    ids[0]
}
pub fn ground(w: &mut JoltWorld, half: f32) -> BodyHandle {
    add_box(
        w,
        Vec3::new(0.0, -0.5, 0.0),
        Quat::IDENTITY,
        Vec3::new(half, 0.5, half),
        MotionType::Static,
    )
}
pub fn ramp(w: &mut JoltWorld, x: f32, angle: f32, length: f32) -> f32 {
    let t = angle.to_radians();
    let direction = Vec3::new(0.0, t.sin(), t.cos());
    let normal = Vec3::new(0.0, t.cos(), -t.sin());
    add_box(
        w,
        Vec3::new(x, 0.0, 0.0) + direction * (length * 0.5) - normal * 0.25,
        Quat::from_rotation_x(-t),
        Vec3::new(1.5, 0.25, length * 0.5),
        MotionType::Static,
    );
    let h = length * t.sin();
    add_box(
        w,
        Vec3::new(x, h * 0.5, length * t.cos() + 5.0),
        Quat::IDENTITY,
        Vec3::new(1.5, h * 0.5, 5.0),
        MotionType::Static,
    );
    h
}
pub fn stairs(w: &mut JoltWorld, x: f32, z: f32) {
    for i in 0..6 {
        let h = (i + 1) as f32 * 0.3;
        add_box(
            w,
            Vec3::new(x, h * 0.5, z + 0.4 * (i as f32 + 0.5)),
            Quat::IDENTITY,
            Vec3::new(1.5, h * 0.5, 0.2),
            MotionType::Static,
        );
    }
    add_box(
        w,
        Vec3::new(x, 0.9, z + 2.4 + 3.0),
        Quat::IDENTITY,
        Vec3::new(1.5, 0.9, 3.0),
        MotionType::Static,
    );
}
pub fn character(w: &mut JoltWorld, position: Vec3) -> CharacterHandle {
    w.backend_mut()
        .create_character(CharacterDesc {
            user_data: 0,
            position: position.to_array(),
            controller: CharacterController::default(),
        })
        .expect("valid character")
}
pub fn input(desired: Vec3) -> CharacterInput {
    CharacterInput {
        desired_velocity: desired.to_array(),
        ..Default::default()
    }
}
pub fn position(w: &JoltWorld, id: BodyHandle) -> Vec3 {
    Vec3::from_array(
        w.backend()
            .body_state(id)
            .expect("live fixture body")
            .pose
            .position,
    )
}
pub fn kinematic(w: &mut JoltWorld, id: BodyHandle, position: Vec3) {
    w.apply(&[BodyCommand::KinematicTarget {
        body: id,
        pose: pose(position, Quat::IDENTITY),
    }])
    .expect("kinematic fixture target");
}
pub fn car(w: &mut JoltWorld, p: Vec3) -> VehicleHandle {
    w.backend_mut()
        .create_vehicle(VehicleDesc::new(
            Vehicle::default(),
            pose(p, Quat::IDENTITY),
            999,
        ))
        .expect("valid car")
}

pub fn bowl(w: &mut JoltWorld, center: Vec3, radius: f32, depth: f32) {
    let shape = bowl_shape(w, center, radius, depth);
    add_body(w, shape, Vec3::ZERO, Quat::IDENTITY, MotionType::Static);
}
/// Cooks the identical bowl geometry for either native or ECS construction.
pub fn bowl_shape(w: &mut JoltWorld, center: Vec3, radius: f32, depth: f32) -> ShapeHandle {
    let (rings, segments) = (24u32, 48u32);
    let mut vertices = vec![center.to_array()];
    for ring in 1..=rings {
        let r = radius * ring as f32 / rings as f32;
        let y = depth * (r / radius).powi(2);
        for s in 0..segments {
            let a = std::f32::consts::TAU * s as f32 / segments as f32;
            vertices.push((center + Vec3::new(r * a.cos(), y, r * a.sin())).to_array());
        }
    }
    let idx = |r: u32, s: u32| 1 + (r - 1) * segments + s % segments;
    let mut indices = Vec::new();
    for s in 0..segments {
        indices.extend([0, idx(1, s + 1), idx(1, s)]);
    }
    for ring in 1..rings {
        for s in 0..segments {
            let (a, b, c, d) = (
                idx(ring, s),
                idx(ring, s + 1),
                idx(ring + 1, s),
                idx(ring + 1, s + 1),
            );
            indices.extend([a, b, d, a, d, c]);
        }
    }
    w.backend_mut()
        .create_shape(Shape::Mesh {
            vertices: &vertices,
            indices: &indices,
        })
        .expect("bowl mesh")
}
pub struct Lcg(u64);
impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e3779b97f4a7c15)
    }
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (((self.0 >> 33) as u32) >> 8) as f32 / (1u32 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}
pub fn grid(
    w: &mut JoltWorld,
    count: usize,
    center: Vec3,
    per_row: usize,
    seed: u64,
) -> Vec<BodyHandle> {
    let bodies = grid_descriptions(w, count, center, per_row, seed);
    let mut ids = Vec::with_capacity(count);
    w.backend_mut()
        .create_bodies(&bodies, &mut ids)
        .expect("grid fits capacity");
    ids
}
/// Uses one seeded description generator for both benchmark integration paths.
pub fn grid_descriptions(
    w: &mut JoltWorld,
    count: usize,
    center: Vec3,
    per_row: usize,
    seed: u64,
) -> Vec<BodyDesc> {
    let cube = w
        .backend_mut()
        .create_shape(Shape::Box {
            half_extents: [0.25; 3],
        })
        .expect("grid box");
    let sphere = w
        .backend_mut()
        .create_shape(Shape::Sphere { radius: 0.25 })
        .expect("grid sphere");
    let mut rng = Lcg::new(seed);
    let span = (per_row as f32 - 1.0) * 0.6 * 0.5;
    let mut bodies = Vec::with_capacity(count);
    for i in 0..count {
        let layer = i / (per_row * per_row);
        let j = i % (per_row * per_row);
        let jitter = Vec3::new(rng.range(-0.05, 0.05), 0.0, rng.range(-0.05, 0.05));
        let axis = Vec3::new(
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0) + 2.0,
        )
        .normalize();
        let rotation = Quat::from_axis_angle(axis, rng.range(0.0, std::f32::consts::PI));
        let p = center
            + Vec3::new(
                (j % per_row) as f32 * 0.6 - span,
                layer as f32 * 0.6,
                (j / per_row) as f32 * 0.6 - span,
            )
            + jitter;
        bodies.push(BodyDesc {
            shape: if i % 2 == 0 { cube } else { sphere },
            user_data: i as u64 + 1,
            pose: pose(p, rotation),
            body: RigidBody {
                motion: MotionType::Dynamic,
                ..Default::default()
            },
            collider: Collider {
                layer: 1,
                friction: 0.5,
                ..Default::default()
            },
            velocity: Velocity::default(),
        });
    }
    bodies
}
pub fn pyramid(w: &mut JoltWorld) -> Vec<BodyHandle> {
    let mut ids = Vec::new();
    for layer in 0..10 {
        let n = 10 - layer;
        let offset = (n as f32 - 1.0) * 0.5;
        for i in 0..n {
            for k in 0..n {
                ids.push(add_box(
                    w,
                    Vec3::new(i as f32 - offset, 0.5 + layer as f32, k as f32 - offset),
                    Quat::IDENTITY,
                    Vec3::splat(0.5),
                    MotionType::Dynamic,
                ));
            }
        }
    }
    ids
}

pub fn hash(
    w: &JoltWorld,
    bodies: &[BodyHandle],
    characters: &[CharacterHandle],
    vehicles: &[VehicleHandle],
) -> u64 {
    let mut h = 14695981039346656037u64;
    let mut word = |n: u32| {
        for b in n.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(1099511628211);
        }
    };
    for id in bodies {
        let s = w.backend().body_state(*id).expect("live hashed body");
        for f in s
            .pose
            .position
            .into_iter()
            .chain(s.pose.rotation.0)
            .chain(s.velocity.linear)
            .chain(s.velocity.angular)
        {
            word(f.to_bits());
        }
        word(u32::from(s.active));
    }
    for id in characters {
        let s = w
            .backend()
            .character_state(*id)
            .expect("live hashed character");
        for f in s
            .position
            .into_iter()
            .chain(s.velocity)
            .chain(s.ground_velocity)
            .chain(s.ground_normal)
        {
            word(f.to_bits());
        }
        word(u32::from(s.grounded));
        word(u32::from(s.crouched));
    }
    for id in vehicles {
        let s = w.backend().vehicle_state(*id).expect("live hashed car");
        word(s.wheels_in_contact);
        word(s.engine_rpm.to_bits());
        word(s.gear as u32);
        for p in s.wheel_positions {
            for f in p {
                word(f.to_bits());
            }
        }
    }
    h
}
