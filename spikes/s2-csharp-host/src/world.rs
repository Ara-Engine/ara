//! The test world (100k entities), its checksum, and Rust ports of the C# benchmark systems.

use crate::schema::{Health, Quat, Transform, Vec3, Velocity};
use bevy_ecs::prelude::*;

/// Entities spawned by [`spawn`].
pub const ENTITIES: usize = 100_000;

/// Spawns `ENTITIES` entities with `Transform` + `Velocity`; every other one also has `Health`,
/// so the world has two archetype tables (two chunks per query on the single-threaded path).
pub fn spawn(world: &mut World) {
    // Deterministic LCG: the world is identical on every run.
    let mut s: u32 = 0x1234_5678;
    let mut rnd = move || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 8) as f32 / (1u32 << 24) as f32
    };
    for i in 0..ENTITIES {
        let t = Transform {
            position: Vec3 { x: rnd() * 800.0 - 400.0, y: rnd() * 50.0, z: rnd() * 800.0 - 400.0 },
            rotation: Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
            scale: 1.0,
        };
        let v = Velocity {
            linear: Vec3 { x: rnd() * 4.0 - 2.0, y: rnd() * 2.0, z: rnd() * 4.0 - 2.0 },
            angular: Vec3 { x: rnd() - 0.5, y: rnd() * 2.0 - 1.0, z: rnd() - 0.5 },
        };
        if i % 2 == 0 {
            world.spawn((t, v, Health { value: rnd() * 100.0, max: 100.0, regen: 1.0 + rnd() * 4.0 }));
        } else {
            world.spawn((t, v));
        }
    }
}

/// FNV-style checksum over every component's bytes, in entity order (deterministic).
pub fn checksum(world: &mut World) -> u64 {
    let mut q = world.query::<(Entity, &Transform, &Velocity, Option<&Health>)>();
    let mut rows: Vec<_> = q.iter(world).map(|(e, t, v, h)| (e.to_bits(), *t, *v, h.copied())).collect();
    rows.sort_unstable_by_key(|r| r.0);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u32| {
        h ^= u64::from(x);
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    for (e, t, v, hp) in rows {
        eat(e as u32);
        let p = &t.position;
        let r = &t.rotation;
        for f in [p.x, p.y, p.z, r.x, r.y, r.z, r.w, t.scale, v.linear.x, v.linear.y, v.linear.z, v.angular.x, v.angular.y, v.angular.z] {
            eat(f.to_bits());
        }
        if let Some(hp) = hp {
            for f in [hp.value, hp.max, hp.regen] {
                eat(f.to_bits());
            }
        }
    }
    h
}

/// Checksum over every component's bytes in query (table) order. Valid for comparing the same world
/// before and after an operation that must not touch it (no structural change in between).
pub fn checksum_fast(world: &mut World) -> u64 {
    let mut q = world.query::<(&Transform, &Velocity, Option<&Health>)>();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u32| {
        h = (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
    };
    for (t, v, hp) in q.iter(world) {
        let (p, r) = (&t.position, &t.rotation);
        for f in [p.x, p.y, p.z, r.x, r.y, r.z, r.w, t.scale, v.linear.x, v.linear.y, v.linear.z, v.angular.x, v.angular.y, v.angular.z] {
            eat(f.to_bits());
        }
        if let Some(hp) = hp {
            eat(hp.value.to_bits());
        }
    }
    h
}

// ---------------------------------------------------------------------------------------------
// Rust ports of `game/Movement.cs` `Integrate` and `game/Forces.cs` `Gravity`, line for line.

#[inline]
fn quat_mul(a: Quat, b: Quat) -> Quat {
    // Hamilton product a*b, the same as System.Numerics `Quaternion operator *`.
    Quat {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    }
}

#[inline]
fn spin(q: Quat, w: Vec3, dt: f32) -> Quat {
    let dq = quat_mul(Quat { x: w.x, y: w.y, z: w.z, w: 0.0 }, q);
    let h = 0.5 * dt;
    let n = Quat { x: q.x + h * dq.x, y: q.y + h * dq.y, z: q.z + h * dq.z, w: q.w + h * dq.w };
    let inv = 1.0 / (n.x * n.x + n.y * n.y + n.z * n.z + n.w * n.w).sqrt();
    Quat { x: n.x * inv, y: n.y * inv, z: n.z * inv, w: n.w * inv }
}

/// Port of `Movement.Integrate`: position += linear * dt; rotation integrated by angular velocity.
pub fn integrate(tr: &mut [Transform], vel: &[Velocity], dt: f32) {
    for (x, v) in tr.iter_mut().zip(vel) {
        x.position.x += v.linear.x * dt;
        x.position.y += v.linear.y * dt;
        x.position.z += v.linear.z * dt;
        x.rotation = spin(x.rotation, v.angular, dt);
    }
}

/// Port of `Forces.Gravity`: gravity plus a ground bounce.
pub fn gravity(vel: &mut [Velocity], tr: &[Transform], dt: f32) {
    const G: f32 = 9.81;
    const RESTITUTION: f32 = 0.5;
    for (v, t) in vel.iter_mut().zip(tr) {
        v.linear.y -= G * dt;
        if t.position.y < 0.0 && v.linear.y < 0.0 {
            v.linear.y = -v.linear.y * RESTITUTION;
        }
    }
}
