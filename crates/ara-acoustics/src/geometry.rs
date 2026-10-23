//! The narrow ray-query seam acoustics needs, plus a self-contained triangle scene.
//!
//! Runtime occlusion goes through [`RayScene`]; the physics adapter (feature `physics`)
//! implements it over `ara-physics` batch ray queries. [`MeshScene`] is a deterministic
//! BVH over triangles used by baking, tests and tools that have no physics world.

use crate::{
    material::AcousticMaterial,
    math::{V3, cross, dot, normalize_or, scale, sub},
};
use ara_audio::{AudioError, Result};
use std::collections::BTreeMap;

/// A world-space query segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Start point (the listener for occlusion).
    pub from: V3,
    /// End point (the source for occlusion).
    pub to: V3,
}

/// One surface crossing along a segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Position along the segment in [0, 1].
    pub fraction: f32,
    /// World-space hit point.
    pub point: V3,
    /// Unit surface normal.
    pub normal: V3,
    /// Stable surface identity (entity / physics user data). Consecutive hits with the
    /// same identity are one blocker: entering and leaving a solid wall counts once.
    pub surface: u64,
    /// Surface material.
    pub material: AcousticMaterial,
}

/// Packed per-segment hit lists; reused between updates without reallocating.
#[derive(Debug, Default, Clone)]
pub struct RayHits {
    hits: Vec<RayHit>,
    offsets: Vec<usize>,
}

impl RayHits {
    /// Removes every segment's hits, keeping capacity.
    pub fn clear(&mut self) {
        self.hits.clear();
        self.offsets.clear();
    }
    /// Starts the next segment's hit list.
    pub fn begin(&mut self) {
        self.offsets.push(self.hits.len());
    }
    /// Appends a hit to the current segment. Call `begin` first.
    pub fn push(&mut self, hit: RayHit) {
        self.hits.push(hit);
    }
    /// Sorts the current segment's hits by fraction, then surface (deterministic order).
    pub fn sort_current(&mut self) {
        let start = self.offsets.last().copied().unwrap_or(0);
        self.hits[start..].sort_by(|a, b| {
            a.fraction
                .total_cmp(&b.fraction)
                .then(a.surface.cmp(&b.surface))
        });
    }
    /// Number of segments.
    pub fn len(&self) -> usize {
        self.offsets.len()
    }
    /// True when no segment has been traced.
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }
    /// Hits for segment `index`, nearest the segment start first.
    pub fn get(&self, index: usize) -> &[RayHit] {
        let Some(&start) = self.offsets.get(index) else {
            return &[];
        };
        let end = self
            .offsets
            .get(index + 1)
            .copied()
            .unwrap_or(self.hits.len());
        &self.hits[start..end]
    }
}

/// Batch segment tracing for acoustics. Runs on a worker, never the audio thread.
pub trait RayScene {
    /// Traces every segment, writing one hit list per segment (sorted by fraction) into `out`.
    fn trace(&mut self, segments: &[Segment], out: &mut RayHits) -> Result<()>;
    /// Human-readable label for a surface identity, used in traces and diagnostics.
    fn label(&self, surface: u64) -> String {
        format!("#{surface}")
    }
}

#[derive(Debug, Clone, Copy)]
struct Triangle {
    indices: [u32; 3],
    surface: u64,
    material: u16,
}

#[derive(Debug, Clone, Copy)]
struct Node {
    min: V3,
    max: V3,
    /// Leaf: first triangle in `order`; interior: right child (left is `index + 1`).
    start: u32,
    /// Leaf triangle count; zero for interior nodes.
    count: u32,
}

/// Closest-hit result from [`MeshScene::closest`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosestHit {
    /// Distance along the unit direction.
    pub distance: f32,
    /// Unit surface normal facing the ray origin.
    pub normal: V3,
    /// Surface identity.
    pub surface: u64,
    /// Surface material.
    pub material: AcousticMaterial,
}

/// Deterministic triangle scene with a median-split BVH. Two-sided surfaces.
#[derive(Debug, Default, Clone)]
pub struct MeshScene {
    vertices: Vec<V3>,
    triangles: Vec<Triangle>,
    materials: Vec<AcousticMaterial>,
    labels: BTreeMap<u64, String>,
    nodes: Vec<Node>,
    order: Vec<u32>,
}

const EPSILON: f32 = 1e-5;

impl MeshScene {
    /// An empty scene.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an indexed triangle mesh with one surface identity and material.
    pub fn add_mesh(
        &mut self,
        vertices: &[V3],
        triangles: &[[u32; 3]],
        surface: u64,
        label: &str,
        material: AcousticMaterial,
    ) -> Result<()> {
        if !material.is_valid()
            || vertices.iter().any(|v| !crate::math::finite(*v))
            || triangles
                .iter()
                .flatten()
                .any(|&i| i as usize >= vertices.len())
        {
            return Err(AudioError::Invalid("mesh vertices, indices or material"));
        }
        let base = u32::try_from(self.vertices.len())
            .map_err(|_| AudioError::Capacity("mesh vertices"))?;
        let material_index = match self.materials.iter().position(|m| *m == material) {
            Some(i) => i,
            None => {
                self.materials.push(material);
                self.materials.len() - 1
            }
        };
        let material =
            u16::try_from(material_index).map_err(|_| AudioError::Capacity("mesh materials"))?;
        self.vertices.extend_from_slice(vertices);
        self.triangles.extend(triangles.iter().map(|t| Triangle {
            indices: t.map(|i| base + i),
            surface,
            material,
        }));
        self.labels.insert(surface, label.to_owned());
        self.nodes.clear();
        Ok(())
    }

    /// Adds an axis-aligned box (walls, crates, rooms: surfaces are two-sided).
    pub fn add_box(
        &mut self,
        min: V3,
        max: V3,
        surface: u64,
        label: &str,
        material: AcousticMaterial,
    ) -> Result<()> {
        let corner = |i: u32| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ]
        };
        let vertices: Vec<V3> = (0..8).map(corner).collect();
        const FACES: [[u32; 4]; 6] = [
            [0, 2, 6, 4],
            [1, 5, 7, 3],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 1, 3, 2],
            [4, 6, 7, 5],
        ];
        let triangles: Vec<[u32; 3]> = FACES
            .iter()
            .flat_map(|[a, b, c, d]| [[*a, *b, *c], [*a, *c, *d]])
            .collect();
        self.add_mesh(&vertices, &triangles, surface, label, material)
    }

    /// Builds the BVH. Call after adding geometry; queries also build it on demand.
    pub fn commit(&mut self) {
        self.order = (0..self.triangles.len() as u32).collect();
        self.nodes.clear();
        if !self.triangles.is_empty() {
            self.build(0, self.triangles.len());
        }
    }

    /// Number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }
    /// Vertex positions, for exporting to external ray tracers (Steam Audio).
    pub fn vertices(&self) -> &[V3] {
        &self.vertices
    }
    /// Triangle vertex indices and material indices, in insertion order.
    pub fn triangles(&self) -> impl Iterator<Item = ([u32; 3], usize)> + '_ {
        self.triangles
            .iter()
            .map(|t| (t.indices, usize::from(t.material)))
    }
    /// Distinct materials referenced by `triangles`.
    pub fn materials(&self) -> &[AcousticMaterial] {
        &self.materials
    }
    /// Scene bounds, or None when empty.
    pub fn bounds(&self) -> Option<(V3, V3)> {
        let first = *self.vertices.first()?;
        Some(self.vertices.iter().fold((first, first), |(lo, hi), v| {
            (
                std::array::from_fn(|i| lo[i].min(v[i])),
                std::array::from_fn(|i| hi[i].max(v[i])),
            )
        }))
    }

    fn centroid(&self, t: u32) -> V3 {
        let [a, b, c] = self.triangles[t as usize]
            .indices
            .map(|i| self.vertices[i as usize]);
        std::array::from_fn(|i| (a[i] + b[i] + c[i]) / 3.0)
    }

    fn build(&mut self, start: usize, end: usize) -> usize {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut cmin = [f32::INFINITY; 3];
        let mut cmax = [f32::NEG_INFINITY; 3];
        for &t in &self.order[start..end] {
            for i in self.triangles[t as usize].indices {
                let v = self.vertices[i as usize];
                for a in 0..3 {
                    min[a] = min[a].min(v[a]);
                    max[a] = max[a].max(v[a]);
                }
            }
            let c = self.centroid(t);
            for a in 0..3 {
                cmin[a] = cmin[a].min(c[a]);
                cmax[a] = cmax[a].max(c[a]);
            }
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            min,
            max,
            start: start as u32,
            count: (end - start) as u32,
        });
        if end - start <= 4 {
            return index;
        }
        let axis = (0..3)
            .max_by(|&a, &b| (cmax[a] - cmin[a]).total_cmp(&(cmax[b] - cmin[b])))
            .unwrap_or(0);
        let mid = (start + end) / 2;
        let mut slice = self.order[start..end].to_vec();
        slice.sort_by(|&a, &b| {
            self.centroid(a)[axis]
                .total_cmp(&self.centroid(b)[axis])
                .then(a.cmp(&b))
        });
        self.order[start..end].copy_from_slice(&slice);
        self.build(start, mid);
        let right = self.build(mid, end);
        self.nodes[index].start = right as u32;
        self.nodes[index].count = 0;
        index
    }

    fn slab(node: &Node, origin: V3, inverse: V3, max_t: f32) -> bool {
        let mut near = 0.0_f32;
        let mut far = max_t;
        for a in 0..3 {
            let t0 = (node.min[a] - origin[a]) * inverse[a];
            let t1 = (node.max[a] - origin[a]) * inverse[a];
            let (lo, hi) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
            // NaN (zero direction on a slab boundary) keeps the box: conservative.
            near = if lo > near { lo } else { near };
            far = if hi < far { hi } else { far };
        }
        near <= far + EPSILON
    }

    fn intersect(&self, t: u32, origin: V3, direction: V3) -> Option<(f32, V3)> {
        let [a, b, c] = self.triangles[t as usize]
            .indices
            .map(|i| self.vertices[i as usize]);
        let e1 = sub(b, a);
        let e2 = sub(c, a);
        let p = cross(direction, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        let s = sub(origin, a);
        let u = dot(s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = cross(s, e1);
        let v = dot(direction, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let distance = dot(e2, q) * inv;
        let mut normal = normalize_or(cross(e1, e2), [0.0, 1.0, 0.0]);
        if dot(normal, direction) > 0.0 {
            normal = scale(normal, -1.0);
        }
        (distance > EPSILON).then_some((distance, normal))
    }

    /// Visits triangles whose node boxes the ray may cross within `max_t`.
    /// Uncommitted scenes fall back to testing every triangle.
    fn visit(&self, origin: V3, direction: V3, max_t: f32, mut f: impl FnMut(u32)) {
        if self.nodes.is_empty() {
            (0..self.triangles.len() as u32).for_each(f);
            return;
        }
        let inverse = direction.map(|d| 1.0 / d);
        let mut stack = [0_u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let index = stack[top];
            let node = &self.nodes[index as usize];
            if !Self::slab(node, origin, inverse, max_t) {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                    f(t);
                }
            } else if top + 2 <= stack.len() {
                stack[top] = node.start;
                stack[top + 1] = index + 1;
                top += 2;
            }
        }
    }

    /// Nearest hit along a unit `direction` within `max_distance`.
    pub fn closest(&self, origin: V3, direction: V3, max_distance: f32) -> Option<ClosestHit> {
        let mut best: Option<(f32, V3, u32)> = None;
        self.visit(origin, direction, max_distance, |t| {
            if let Some((d, n)) = self.intersect(t, origin, direction)
                && d <= max_distance
                && best.is_none_or(|(b, _, bt)| d < b || (d == b && t < bt))
            {
                best = Some((d, n, t));
            }
        });
        best.map(|(distance, normal, t)| {
            let tri = self.triangles[t as usize];
            ClosestHit {
                distance,
                normal,
                surface: tri.surface,
                material: self.materials[usize::from(tri.material)],
            }
        })
    }

    fn trace_one(&self, segment: Segment, out: &mut RayHits) {
        out.begin();
        let delta = sub(segment.to, segment.from);
        let length = crate::math::length(delta);
        if length <= EPSILON {
            return;
        }
        let direction = scale(delta, 1.0 / length);
        self.visit(segment.from, direction, length, |t| {
            if let Some((d, normal)) = self.intersect(t, segment.from, direction)
                && d < length - EPSILON
            {
                let tri = self.triangles[t as usize];
                out.push(RayHit {
                    fraction: d / length,
                    point: crate::math::add(segment.from, scale(direction, d)),
                    normal,
                    surface: tri.surface,
                    material: self.materials[usize::from(tri.material)],
                });
            }
        });
        out.sort_current();
    }
}

impl RayScene for MeshScene {
    fn trace(&mut self, segments: &[Segment], out: &mut RayHits) -> Result<()> {
        if self.nodes.is_empty() && !self.triangles.is_empty() {
            self.commit();
        }
        out.clear();
        for segment in segments {
            if !crate::math::finite(segment.from) || !crate::math::finite(segment.to) {
                return Err(AudioError::Invalid("non-finite ray segment"));
            }
            self.trace_one(*segment, out);
        }
        Ok(())
    }
    fn label(&self, surface: u64) -> String {
        self.labels
            .get(&surface)
            .cloned()
            .unwrap_or_else(|| format!("#{surface}"))
    }
}
