//! Fixed-tick export conformance data. Authoritative values live in ECS components.
use crate::abi::{Chunk, Transform, Velocity};
use crate::runtime::{CHUNK_ROWS, Spawner};
use bevy_ecs::prelude::Component;

/// ECS component containing conformance batch columns for one archetype: `Transform` + `Velocity`.
#[derive(Clone, Component)]
pub struct Columns {
    /// Column 0.
    pub transforms: Vec<Transform>,
    /// Column 1.
    pub velocities: Vec<Velocity>,
}

impl Columns {
    /// Deterministic spawn from the scene's seed (SplitMix64; no global RNG).
    pub(crate) fn spawn(s: Spawner) -> Self {
        let mut state = s.seed;
        let mut next = move || {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) >> 40) as f32 / (1u32 << 24) as f32
        };
        let mut transforms = Vec::with_capacity(s.count);
        let mut velocities = Vec::with_capacity(s.count);
        for _ in 0..s.count {
            transforms.push(Transform {
                x: (next() * 2.0 - 1.0) * s.extent,
                y: next() * s.extent,
                z: (next() * 2.0 - 1.0) * s.extent,
                yaw: next() * std::f32::consts::TAU,
            });
            velocities.push(Velocity {
                x: (next() * 2.0 - 1.0) * s.speed,
                y: (next() * 2.0 - 1.0) * s.speed,
                z: (next() * 2.0 - 1.0) * s.speed,
                yaw_rate: next() * 2.0 - 1.0,
            });
        }
        Self {
            transforms,
            velocities,
        }
    }

    /// Chunk views over the columns. Valid until the columns are resized or moved.
    pub(crate) fn chunks(&mut self) -> Vec<Chunk> {
        let t = self.transforms.as_mut_ptr();
        let v = self.velocities.as_mut_ptr();
        (0..self.transforms.len())
            .step_by(CHUNK_ROWS)
            .map(|start| Chunk {
                len: CHUNK_ROWS.min(self.transforms.len() - start) as u32,
                column_count: 2,
                // SAFETY: `start < len`, so both offsets stay inside their allocations.
                col0: unsafe { t.add(start) }.cast(),
                // SAFETY: as above; both columns have the same length.
                col1: unsafe { v.add(start) }.cast(),
            })
            .collect()
    }
}

/// The Rust twin of C# `Movement.Run`; identical operation order, so results must match bit-for-bit.
pub fn movement_reference(world: &mut Columns, dt: f32) {
    let g = 9.81f32 * dt;
    for (t, v) in world.transforms.iter_mut().zip(world.velocities.iter_mut()) {
        v.y -= g;
        t.x += v.x * dt;
        t.y += v.y * dt;
        t.z += v.z * dt;
        if t.y < 0.0 {
            t.y = -t.y;
            v.y = -v.y * 0.8;
        }
        t.yaw += v.yaw_rate * dt;
    }
}

/// FNV-1a 64 over the bit patterns of every `Transform` float, in row order.
pub fn checksum(transforms: &[Transform]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for t in transforms {
        for w in [t.x, t.y, t.z, t.yaw] {
            h ^= u64::from(w.to_bits());
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}
