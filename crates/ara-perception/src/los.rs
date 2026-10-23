//! Line-of-sight providers: open space, closures and physics rays.
use ara_physics::{PhysicsBackend, PhysicsWorld, Query, QueryFilter, QueryResults};

/// Answers line-of-sight queries for perception and EQS. Implementations must
/// be deterministic and thread-safe: perceivers query in parallel.
pub trait LineOfSight: Sync {
    /// True when nothing blocks the segment `from` -> `to`. `viewer` and `target`
    /// are stable IDs so implementations can ignore the two bodies themselves.
    fn clear(&self, from: [f32; 3], to: [f32; 3], viewer: u64, target: u64) -> bool;
}

/// No occluders: every line is clear (tests, open fields).
pub struct OpenSpace;
impl LineOfSight for OpenSpace {
    fn clear(&self, _: [f32; 3], _: [f32; 3], _: u64, _: u64) -> bool {
        true
    }
}

impl<F: Fn([f32; 3], [f32; 3], u64, u64) -> bool + Sync> LineOfSight for F {
    fn clear(&self, from: [f32; 3], to: [f32; 3], viewer: u64, target: u64) -> bool {
        self(from, to, viewer, target)
    }
}

/// Physics ray line of sight. Bodies must carry the perception stable ID as
/// their `user_data` (the `PhysicsId` value), so the viewer's and the target's
/// own colliders are skipped. Sensors never occlude.
pub struct PhysicsLineOfSight<'a, B: PhysicsBackend> {
    /// World to cast against (read-only; queries are deterministic and sorted).
    pub physics: &'a PhysicsWorld<B>,
    /// Collision layers that block sight.
    pub layer_mask: u32,
}

impl<B: PhysicsBackend> LineOfSight for PhysicsLineOfSight<'_, B> {
    fn clear(&self, from: [f32; 3], to: [f32; 3], viewer: u64, target: u64) -> bool {
        let query = Query::Ray {
            origin: from,
            displacement: std::array::from_fn(|i| to[i] - from[i]),
            filter: QueryFilter {
                layer_mask: self.layer_mask,
                sensors: false,
                ignore: None,
            },
        };
        let mut out = QueryResults::default();
        if self.physics.backend().queries(&[query], &mut out).is_err() {
            return false; // A failed query never grants sight.
        }
        out.get(0)
            .iter()
            .all(|hit| hit.user_data == viewer || hit.user_data == target || hit.fraction >= 0.999)
    }
}
