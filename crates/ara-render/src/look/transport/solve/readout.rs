//! Surface identity lookup and smooth, material/instance-separated patch readout.
//! Coverage comes from A2's source-triangle barycentrics; a pixel never chooses a
//! patch on the other side of a thin wall merely because its centroid is nearer.

use crate::look::transport::patches::{PatchBuffers, PatchId};
use ara_patch_charts::Charts;
use glam::{Vec2, Vec3};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
struct Coverage {
    patch: u32,
    bary: [[f32; 2]; 3],
}

/// One mapping per resident chart instance; it is rebuilt on topology publication.
#[derive(Default)]
pub(crate) struct SurfaceMap {
    triangles: BTreeMap<(u64, u32), Vec<Coverage>>,
}

impl SurfaceMap {
    pub fn add(
        &mut self,
        instance: u64,
        incarnation: u32,
        charts: &Charts,
        patches: &PatchBuffers,
    ) {
        for (local, patch) in charts.patches.iter().enumerate() {
            let Some(dense) = patches.index(PatchId {
                instance,
                incarnation,
                local: local as u32,
            }) else {
                continue;
            };
            for fragment in charts.coverage(patch) {
                self.triangles
                    .entry((instance, fragment.triangle))
                    .or_default()
                    .push(Coverage {
                        patch: dense,
                        bary: fragment.barycentric,
                    });
            }
        }
    }

    pub fn patch(&self, instance: u64, primitive: u32, barycentric: [f32; 2]) -> Option<u32> {
        let p = Vec2::from(barycentric);
        self.triangles
            .get(&(instance, primitive))?
            .iter()
            .find(|c| {
                let [a, b, d] = c.bary.map(Vec2::from);
                let cross = |u: Vec2, v: Vec2| u.x * v.y - u.y * v.x;
                let area = cross(b - a, d - a);
                if area == 0. {
                    return false;
                }
                let u = cross(b - p, d - p) / area;
                let v = cross(d - p, a - p) / area;
                let w = 1. - u - v;
                u >= -2e-5 && v >= -2e-5 && w >= -2e-5
            })
            .map(|c| c.patch)
    }

    pub fn resident_bytes(&self) -> usize {
        self.triangles
            .values()
            .map(|v| v.capacity() * size_of::<Coverage>() + 80)
            .sum()
    }
}

/// Compact per-patch interpolation neighbours, constrained to the same material,
/// instance, near-coplanar normal and plane. Built once; readout only visits the
/// caller's four neighbouring patches. This is spatial basis interpolation, not
/// a blur pass over the integral or over a screen image.
pub(crate) struct Readout {
    neighbours: Vec<Vec<u32>>,
}

impl Readout {
    pub fn new(patches: &PatchBuffers) -> Self {
        let mut cells: BTreeMap<(u64, [i32; 3]), Vec<u32>> = BTreeMap::new();
        for (i, stamp) in patches.stamps.iter().enumerate() {
            let cell = (Vec3::from(patches.position(i as u32)) / 0.25)
                .floor()
                .as_ivec3()
                .to_array();
            cells
                .entry((stamp.id.instance, cell))
                .or_default()
                .push(i as u32);
        }
        let mut neighbours = Vec::with_capacity(patches.len());
        for (i, stamp) in patches.stamps.iter().enumerate() {
            let position = Vec3::from(patches.position(i as u32));
            let normal = Vec3::from_array(patches.normal.each_ref().map(|v| v[i]));
            let cell = (position / 0.25).floor().as_ivec3();
            let mut local = Vec::new();
            for z in -1..=1 {
                for y in -1..=1 {
                    for x in -1..=1 {
                        if let Some(indices) = cells.get(&(
                            stamp.id.instance,
                            (cell + glam::IVec3::new(x, y, z)).to_array(),
                        )) {
                            for &j in indices {
                                let n = Vec3::from_array(
                                    patches.normal.each_ref().map(|v| v[j as usize]),
                                );
                                let delta = Vec3::from(patches.position(j)) - position;
                                if patches.material[i] == patches.material[j as usize]
                                    && n.dot(normal) > 0.999
                                    && delta.dot(normal).abs() < 1e-4
                                    && delta.length_squared() <= 0.36f32.powi(2)
                                {
                                    local.push(j);
                                }
                            }
                        }
                    }
                }
            }
            local.sort_unstable();
            local.dedup();
            neighbours.push(local);
        }
        Self { neighbours }
    }

    /// Compact hat basis in the surface tangent frame; normal and material edges
    /// never share values. Channel combination is called only for touched patches.
    pub fn sample(
        &self,
        patches: &PatchBuffers,
        owner: u32,
        position: Vec3,
        mut read: impl FnMut(u32) -> Vec3,
    ) -> Vec3 {
        let normal = Vec3::from_array(patches.normal.each_ref().map(|v| v[owner as usize]));
        let tangent = normal.any_orthonormal_vector();
        let bitangent = normal.cross(tangent);
        let mut total = 0.;
        let mut result = Vec3::ZERO;
        for &index in &self.neighbours[owner as usize] {
            let delta = position - Vec3::from(patches.position(index));
            let weight = (1. - delta.dot(tangent).abs() / 0.25).max(0.)
                * (1. - delta.dot(bitangent).abs() / 0.25).max(0.);
            if weight > 0. {
                result += read(index) * weight;
                total += weight;
            }
        }
        if total > 0. {
            result / total
        } else {
            read(owner)
        }
    }

    pub fn resident_bytes(&self) -> usize {
        self.neighbours.capacity() * size_of::<Vec<u32>>()
            + self
                .neighbours
                .iter()
                .map(|v| v.capacity() * 4)
                .sum::<usize>()
    }
}
