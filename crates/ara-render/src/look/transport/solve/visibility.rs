//! CPU adapter of A1's threaded TLAS and CWBVH traversal. It preserves instance
//! keys/source primitives, including alpha tests supplied by the material service.

use crate::look::trace::{InstanceKey, TraceMesh, TraceScene};
use ara_bvh::{
    Ray, RayFlags,
    tlas::{END, LEAF},
};
use glam::Vec3;

#[derive(Clone, Copy, Debug)]
pub(crate) struct SurfaceHit {
    pub instance: InstanceKey,
    pub id: u64,
    pub primitive: u32,
    pub barycentric: [f32; 2],
    pub distance: f32,
    pub front: bool,
}

/// Cached mesh-slot lookup avoids searching the asset map at every TLAS leaf.
pub(crate) struct CpuTrace<'a> {
    pub scene: &'a TraceScene,
    meshes: Vec<Option<&'a TraceMesh>>,
}

impl<'a> CpuTrace<'a> {
    pub fn new(scene: &'a TraceScene) -> Self {
        let mut meshes = vec![None; scene.blas_slot_count()];
        for (slot, mesh) in scene.blas_slots() {
            meshes[slot as usize] = Some(mesh);
        }
        Self { scene, meshes }
    }

    /// `accept` performs alpha/transmission/body-policy tests on actual source
    /// triangles. Returning false continues traversal, including within a BLAS.
    pub fn closest(&self, ray: Ray, accept: impl FnMut(SurfaceHit) -> bool) -> Option<SurfaceHit> {
        self.closest_where(ray, |_| true, accept)
    }

    /// Keep TLAS pruning but traverse triangles only in the affected instances.
    /// Cached visibility repair uses this for exact local blocker tests.
    pub fn closest_where(
        &self,
        ray: Ray,
        mut instance_filter: impl FnMut(u64) -> bool,
        mut accept: impl FnMut(SurfaceHit) -> bool,
    ) -> Option<SurfaceHit> {
        let mut node = if self.scene.tlas().nodes.is_empty() {
            END
        } else {
            0
        };
        let mut best = None;
        let mut maximum = ray.t_max;
        while node != END {
            let n = &self.scene.tlas().nodes[node as usize];
            if !box_hit(
                ray.origin,
                ray.direction,
                Vec3::from(n.min),
                Vec3::from(n.max),
                ray.t_min,
                maximum,
            ) {
                node = n.skip;
                continue;
            }
            if n.first & LEAF == 0 {
                node = n.first;
                continue;
            }
            let slot = n.first & !LEAF;
            node = n.skip;
            let Some(instance) = self.scene.slot(slot) else {
                continue;
            };
            if instance.instance.mask == 0 || !instance_filter(instance.id) {
                continue;
            }
            let Some(key) = self.scene.blas_key(instance.instance.mesh) else {
                continue;
            };
            let Some(mesh) = self.meshes[key.slot as usize] else {
                continue;
            };
            let inverse = instance.instance.world.inverse();
            let local = Ray {
                origin: inverse.transform_point3(ray.origin),
                direction: inverse.transform_vector3(ray.direction),
                t_min: ray.t_min,
                t_max: maximum,
            };
            let mirrored = instance.instance.world.matrix3.determinant() < 0.;
            let convert = |hit: &ara_bvh::Hit| SurfaceHit {
                instance: instance.key,
                id: instance.id,
                primitive: hit.primitive,
                barycentric: hit.barycentrics,
                distance: hit.t,
                front: hit.front != mirrored,
            };
            if let Some(hit) = mesh
                .blas
                .intersect_filtered(&local, RayFlags::NONE, |h| accept(convert(h)))
            {
                maximum = hit.t;
                best = Some(convert(&hit));
            }
        }
        best
    }
}

fn box_hit(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3, mut lo: f32, mut hi: f32) -> bool {
    for axis in 0..3 {
        if direction[axis] == 0. {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return false;
            }
        } else {
            let a = (min[axis] - origin[axis]) / direction[axis];
            let b = (max[axis] - origin[axis]) / direction[axis];
            lo = lo.max(a.min(b));
            hi = hi.min(a.max(b));
            if lo > hi {
                return false;
            }
        }
    }
    true
}
