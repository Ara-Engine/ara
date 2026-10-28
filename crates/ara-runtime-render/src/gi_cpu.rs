//! CPU probe-surface acquisition: the bake for devices without hardware ray
//! queries, and every incremental update after an edit. [`Tracer`] is a two-level
//! BVH: one binned-SAH BVH per unique geometry in model space (the finest LOD
//! level within a triangle budget) under a top level over the static instances'
//! world bounds; rays enter an instance through its inverse transform. Probes are
//! traced in parallel with the rays, relocation and packing of the GPU bake
//! (`probe-surfaces.slang`), so every GPU gets the same probe GI.
//!
//! An edit only moves, adds or drops instances ([`Tracer::adopt`],
//! [`Tracer::edit`]) and re-traces the probes it can change ([`affected`],
//! [`rebake`]). Deforming (skinned) geometry and dynamic entities never enter the
//! surfaces. Materials use their factors times each map's mean texel;
//! alpha-tested surfaces count as opaque.
use anyhow::{Context, Result, ensure};
use ara_render::{
    instanced::{Geometry, Scene},
    look::gi::{ProbeLattice, ProbeLayout, ProbeSurfaces, STATIC_VECTORS, SURFACE_TEXELS},
};
use ara_render_fidelity::probes::ProbeVolume;
use glam30::{Mat3, Mat4, Vec2, Vec3};
use std::collections::{BTreeSet, HashMap};

/// Longest probe ray, matching the GPU bake.
const RANGE: f32 = 200.;
/// Triangles per mesh leaf.
const LEAF: usize = 4;
/// Instances per top-level leaf.
const TOP_LEAF: usize = 2;
const BINS: usize = 16;
const EMPTY: (Vec3, Vec3) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));

/// Probes a bake worker takes at a time.
const BATCH: usize = 32;

/// Threads a bake, re-trace or mesh build fans out to: half the logical cores, so
/// the render thread and the driver keep theirs (an all-core bake starved
/// presentation into 100+ ms frames on Outpost while it ran).
fn worker_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1))
}

/// Diffuse albedo (metal removed), emissive radiance and the share of rays an
/// alpha-tested or blended surface lets through.
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) struct Shading {
    pub albedo: Vec3,
    pub emissive: Vec3,
    /// Share of rays passing through, as the ray-traced bake's any-hit test
    /// (alpha mask coverage, or one minus blend alpha); 0 is opaque.
    pub cutout: f32,
}

/// Whether the ray `seed` passes a triangle of a surface with `cutout`: a
/// stable hash per ray and triangle, so bakes stay deterministic.
fn passes(cutout: f32, seed: u32, triangle: u32) -> bool {
    if cutout <= 0. {
        return false;
    }
    let mut h = seed ^ triangle.wrapping_mul(0x9e37_79b9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    ((h >> 8) as f32) < cutout * (1u32 << 24) as f32
}

/// A model-space triangle.
#[derive(Clone, Copy)]
struct Triangle {
    a: Vec3,
    e1: Vec3,
    e2: Vec3,
    /// Geometric normal oriented with the vertex normals.
    normal: Vec3,
    /// Mean of the vertices' terrain splat bytes (zero for models).
    weights: [u8; 4],
}

impl Triangle {
    fn bounds(&self) -> (Vec3, Vec3) {
        let (b, c) = (self.a + self.e1, self.a + self.e2);
        (self.a.min(b).min(c), self.a.max(b).max(c))
    }
}

#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: first item; interior: left child (right is `first + 1`).
    first: u32,
    /// Leaf item count; zero for interior nodes.
    count: u32,
}

fn union((a, b): (Vec3, Vec3), (c, d): (Vec3, Vec3)) -> (Vec3, Vec3) {
    (a.min(c), b.max(d))
}

fn area(lo: Vec3, hi: Vec3) -> f32 {
    let d = (hi - lo).max(Vec3::ZERO);
    d.x * d.y + d.y * d.z + d.z * d.x
}

/// World bounds of model bounds `local` under `world` (all eight corners).
fn world_bounds(world: Mat4, (lo, hi): (Vec3, Vec3)) -> (Vec3, Vec3) {
    (0..8).fold(EMPTY, |acc, corner| {
        let pick = |bit: usize, axis: usize| {
            if corner >> bit & 1 == 0 {
                lo[axis]
            } else {
                hi[axis]
            }
        };
        let p = world.transform_point3(Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)));
        union(acc, (p, p))
    })
}

/// Binned surface-area-heuristic build over item bounds: reorders `items` so
/// every leaf is a contiguous run (of at most `leaf` items unless they cannot
/// be split).
fn build<T>(items: &mut [T], bound: impl Fn(&T) -> (Vec3, Vec3), leaf: usize) -> Vec<Node> {
    let span = |items: &[T]| items.iter().fold(EMPTY, |acc, t| union(acc, bound(t)));
    let centre = |t: &T| {
        let (lo, hi) = bound(t);
        (lo + hi) * 0.5
    };
    let mut nodes = Vec::with_capacity(items.len() / leaf.max(1) * 2 + 1);
    if items.is_empty() {
        return nodes;
    }
    let (lo, hi) = span(items);
    nodes.push(Node {
        min: lo,
        max: hi,
        first: 0,
        count: items.len() as u32,
    });
    let mut stack = vec![0usize];
    while let Some(index) = stack.pop() {
        let node = nodes[index];
        let (start, count) = (node.first as usize, node.count as usize);
        if count <= leaf {
            continue;
        }
        let run = &mut items[start..start + count];
        let (clo, chi) = run
            .iter()
            .fold(EMPTY, |(lo, hi), t| (lo.min(centre(t)), hi.max(centre(t))));
        let extent = chi - clo;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        if extent[axis] <= 1e-6 {
            continue;
        }
        let bin = |t: &T| {
            (((centre(t)[axis] - clo[axis]) / extent[axis] * BINS as f32) as usize).min(BINS - 1)
        };
        let mut bins = [(0usize, EMPTY); BINS];
        for t in run.iter() {
            let b = &mut bins[bin(t)];
            *b = (b.0 + 1, union(b.1, bound(t)));
        }
        // Sweep the split planes for the cheapest SAH cost.
        let mut best = (f32::INFINITY, 0usize);
        for split in 1..BINS {
            let side = |range: std::ops::Range<usize>| {
                bins[range]
                    .iter()
                    .fold((0usize, EMPTY), |(n, b), c| (n + c.0, union(b, c.1)))
            };
            let (ln, (llo, lhi)) = side(0..split);
            let (rn, (rlo, rhi)) = side(split..BINS);
            if ln == 0 || rn == 0 {
                continue;
            }
            let cost = ln as f32 * area(llo, lhi) + rn as f32 * area(rlo, rhi);
            if cost < best.0 {
                best = (cost, split);
            }
        }
        if best.0 >= count as f32 * area(node.min, node.max) {
            continue;
        }
        // Partition the run by bin.
        let (mut i, mut j) = (0usize, count);
        while i < j {
            if bin(&run[i]) < best.1 {
                i += 1;
            } else {
                j -= 1;
                run.swap(i, j);
            }
        }
        if i == 0 || i == count {
            continue;
        }
        let left = nodes.len();
        for (first, n) in [(start, i), (start + i, count - i)] {
            let (lo, hi) = span(&items[first..first + n]);
            nodes.push(Node {
                min: lo,
                max: hi,
                first: first as u32,
                count: n as u32,
            });
        }
        nodes[index].first = left as u32;
        nodes[index].count = 0;
        stack.extend([left, left + 1]);
    }
    nodes
}

/// Closest-first traversal of `nodes` for `origin + t * direction`, t below
/// `maximum`: `leaf(first, count, limit)` intersects a leaf's items and returns
/// a hit distance under `limit`, which then bounds the search. Returns the
/// closest distance found, or `maximum`.
fn traverse(
    nodes: &[Node],
    origin: Vec3,
    direction: Vec3,
    maximum: f32,
    mut leaf: impl FnMut(u32, u32, f32) -> Option<f32>,
) -> f32 {
    let inverse = direction.recip();
    let slab = |node: &Node, limit: f32| {
        let t0 = (node.min - origin) * inverse;
        let t1 = (node.max - origin) * inverse;
        let near = t0.min(t1).max_element().max(0.);
        let far = t0.max(t1).min_element().min(limit);
        (near <= far).then_some(near)
    };
    let mut best = maximum;
    if nodes.is_empty() || slab(&nodes[0], best).is_none() {
        return best;
    }
    let mut stack = [0u32; 64];
    let mut depth = 1usize;
    while depth > 0 {
        depth -= 1;
        let node = &nodes[stack[depth] as usize];
        if slab(node, best).is_none() {
            continue;
        }
        if node.count > 0 {
            if let Some(t) = leaf(node.first, node.count, best) {
                best = t;
            }
            continue;
        }
        // Visit the nearer child first.
        let (l, r) = (node.first, node.first + 1);
        let (dl, dr) = (
            slab(&nodes[l as usize], best),
            slab(&nodes[r as usize], best),
        );
        let mut push = |child: u32| {
            if depth < stack.len() {
                stack[depth] = child;
                depth += 1;
            }
        };
        match (dl, dr) {
            (Some(a), Some(b)) if a <= b => {
                push(r);
                push(l);
            }
            (Some(_), Some(_)) => {
                push(l);
                push(r);
            }
            (Some(_), None) => push(l),
            (None, Some(_)) => push(r),
            (None, None) => {}
        }
    }
    best
}

/// One geometry's model-space triangles and their BVH.
pub(crate) struct Mesh {
    nodes: Vec<Node>,
    triangles: Vec<Triangle>,
}

impl Mesh {
    fn new(mut triangles: Vec<Triangle>) -> Self {
        let nodes = build(&mut triangles, Triangle::bounds, LEAF);
        Self { nodes, triangles }
    }

    /// The geometry's triangles at LOD `level` (or its coarsest).
    pub(crate) fn of(geometry: &Geometry, level: usize) -> Self {
        let Some(lod) = geometry
            .lods
            .get(level.min(geometry.lods.len().saturating_sub(1)))
        else {
            return Self::new(Vec::new());
        };
        let mut triangles = Vec::with_capacity(lod.indices.len() / 3);
        for tri in lod.indices.as_chunks::<3>().0 {
            let Some([a, b, c]) = tri
                .iter()
                .map(|&i| geometry.vertices.get(i as usize))
                .collect::<Option<Vec<_>>>()
                .and_then(|v| <[_; 3]>::try_from(v).ok())
            else {
                continue;
            };
            let [pa, pb, pc] = [a, b, c].map(|v| Vec3::from_array(v.position));
            let (e1, e2) = (pb - pa, pc - pa);
            let mut normal = e1.cross(e2).normalize_or_zero();
            if normal == Vec3::ZERO {
                continue;
            }
            let shading = Vec3::from_array(a.normal)
                + Vec3::from_array(b.normal)
                + Vec3::from_array(c.normal);
            if normal.dot(shading) < 0. {
                normal = -normal;
            }
            let splat = [a, b, c].map(|v| v.terrain_weights.to_le_bytes());
            triangles.push(Triangle {
                a: pa,
                e1,
                e2,
                normal,
                weights: std::array::from_fn(|i| {
                    (splat.iter().map(|w| u32::from(w[i])).sum::<u32>() / 3) as u8
                }),
            });
        }
        Self::new(triangles)
    }

    /// Model bounds (empty when the mesh has no triangles).
    pub(crate) fn bounds(&self) -> (Vec3, Vec3) {
        self.nodes.first().map_or(EMPTY, |n| (n.min, n.max))
    }

    /// Closest hit with 0.001 < t < `limit`: distance, model-space normal and the
    /// triangle's splat weights. `direction` need not be unit length; t is in its
    /// units.
    pub(crate) fn trace(
        &self,
        origin: Vec3,
        direction: Vec3,
        limit: f32,
    ) -> Option<(f32, Vec3, [u8; 4])> {
        self.trace_cut(origin, direction, limit, 0., 0)
    }

    /// [`Self::trace`] through a surface with `cutout`: the ray `seed` passes the
    /// triangles [`passes`] picks.
    fn trace_cut(
        &self,
        origin: Vec3,
        direction: Vec3,
        limit: f32,
        cutout: f32,
        seed: u32,
    ) -> Option<(f32, Vec3, [u8; 4])> {
        let mut normal = None;
        let distance = traverse(
            &self.nodes,
            origin,
            direction,
            limit,
            |first, count, limit| {
                let mut closest = None;
                let mut limit = limit;
                let leaf = &self.triangles[first as usize..(first + count) as usize];
                for (index, t) in (first..).zip(leaf) {
                    // Moller-Trumbore.
                    let p = direction.cross(t.e2);
                    let det = t.e1.dot(p);
                    if det.abs() < 1e-10 {
                        continue;
                    }
                    let inv = 1. / det;
                    let s = origin - t.a;
                    let u = s.dot(p) * inv;
                    if !(0. ..=1.).contains(&u) {
                        continue;
                    }
                    let q = s.cross(t.e1);
                    let v = direction.dot(q) * inv;
                    if v < 0. || u + v > 1. {
                        continue;
                    }
                    let d = t.e2.dot(q) * inv;
                    if d > 0.001 && d < limit && !passes(cutout, seed, index) {
                        limit = d;
                        closest = Some(d);
                        normal = Some((t.normal, t.weights));
                    }
                }
                closest
            },
        );
        normal.map(|(n, w)| (distance, n, w))
    }
}

/// A terrain instance's splat layers (`MaterialExtras`): its layer materials'
/// albedos, blended per hit like native-raster's `terrainSurface`.
#[derive(Clone, Copy, PartialEq)]
struct Layers {
    albedo: [Vec3; 4],
    /// Slots in use (`terrain_layers`); later slots weigh nothing.
    count: usize,
}

impl Layers {
    /// The albedo where the splat bytes are `weights`: unused slots dropped, the
    /// rest normalised, layer 0 alone when nothing remains.
    fn albedo(&self, weights: [u8; 4]) -> Vec3 {
        let mut w = weights.map(f32::from);
        w[self.count.min(4)..].fill(0.);
        let sum: f32 = w.iter().sum();
        if sum <= 0. {
            return self.albedo[0];
        }
        self.albedo.iter().zip(w).map(|(&a, w)| a * (w / sum)).sum()
    }
}

/// One scene instance as the tracer sees it.
struct Placed {
    entity: u64,
    /// Geometry identity (`Geometry::key`).
    key: [u8; 32],
    /// Index into `Tracer::meshes`; meaningless for deforming geometry.
    mesh: usize,
    world: Mat4,
    inverse: Mat4,
    /// Inverse transpose of `world`'s linear part.
    normal: Mat3,
    shading: Shading,
    /// Terrain splat layers (their hits replace `shading.albedo`).
    layers: Option<Layers>,
    /// Model bounds of the mesh.
    local: (Vec3, Vec3),
    /// World bounds.
    bounds: (Vec3, Vec3),
    deforming: bool,
    /// Traced into the probe surfaces: static, rigid and invertible.
    included: bool,
}

impl Placed {
    fn place(&mut self, world: Mat4) {
        self.world = world;
        self.inverse = world.inverse();
        self.normal = Mat3::from_mat4(world).inverse().transpose();
        self.bounds = world_bounds(world, self.local);
    }

    fn eligible(&self, dynamic: &BTreeSet<u64>) -> bool {
        !self.deforming
            && !dynamic.contains(&self.entity)
            && self.world.determinant().abs() > 1e-12
            && self.local.0.cmple(self.local.1).all()
    }
}

/// A closest hit in world space.
struct Hit {
    distance: f32,
    /// Unit world normal, oriented with the vertex normals.
    normal: Vec3,
    shading: Shading,
}

/// Two-level BVH over a scene's static instances; it persists across edits so
/// moves rebuild only the top level and new content only its new geometry.
pub(crate) struct Tracer {
    /// LOD level fixed by the first scene (meshes are reused across edits).
    level: Option<usize>,
    meshes: Vec<Mesh>,
    keys: HashMap<[u8; 32], usize>,
    placed: Vec<Placed>,
    top: Vec<Node>,
    /// `placed` indices of included instances, in top-level leaf order.
    order: Vec<u32>,
}

impl Tracer {
    /// A tracer over `scene`; `shading` is indexed by material, `dynamic`
    /// entities stay out.
    pub(crate) fn new(scene: &Scene, shading: &[Shading], dynamic: &BTreeSet<u64>) -> Self {
        let mut tracer = Self {
            level: None,
            meshes: Vec::new(),
            keys: HashMap::new(),
            placed: Vec::new(),
            top: Vec::new(),
            order: Vec::new(),
        };
        tracer.adopt(scene, shading, dynamic);
        tracer
    }

    /// Adopts a new content generation: meshes are reused by geometry key (new
    /// ones built in parallel) and instances matched by entity. Returns the world
    /// bounds whose traced contents changed (old and new places).
    pub(crate) fn adopt(
        &mut self,
        scene: &Scene,
        shading: &[Shading],
        dynamic: &BTreeSet<u64>,
    ) -> Vec<(Vec3, Vec3)> {
        let level = *self.level.get_or_insert_with(|| lod_level(scene));
        let mut old: Vec<Option<Mesh>> = std::mem::take(&mut self.meshes)
            .into_iter()
            .map(Some)
            .collect();
        let mut keys = HashMap::new();
        let mut meshes = Vec::new();
        let mut missing = Vec::new();
        for geometry in &scene.geometry {
            if geometry.deforming || keys.contains_key(&geometry.key) {
                continue;
            }
            keys.insert(geometry.key, meshes.len());
            let reused = self
                .keys
                .get(&geometry.key)
                .and_then(|&i| old.get_mut(i))
                .and_then(Option::take);
            if reused.is_none() {
                missing.push((meshes.len(), geometry));
            }
            meshes.push(reused);
        }
        let threads = worker_threads();
        let chunk = missing.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            let jobs: Vec<_> = missing
                .chunks(chunk)
                .map(|jobs| {
                    scope.spawn(move || {
                        jobs.iter()
                            .map(|&(i, geometry)| (i, Mesh::of(geometry, level)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            for job in jobs {
                // A panicked build leaves its meshes empty (traced as missing).
                for (i, mesh) in job.join().unwrap_or_default() {
                    meshes[i] = Some(mesh);
                }
            }
        });
        self.meshes = meshes
            .into_iter()
            .map(|m| m.unwrap_or_else(|| Mesh::new(Vec::new())))
            .collect();
        self.keys = keys;

        let mut previous: HashMap<u64, Placed> = std::mem::take(&mut self.placed)
            .into_iter()
            .map(|p| (p.entity, p))
            .collect();
        let mut changed = Vec::new();
        for instance in &scene.instances {
            let Some(geometry) = scene.geometry.get(instance.geometry) else {
                continue;
            };
            let mesh = self.keys.get(&geometry.key).copied();
            let local = mesh.map_or((geometry.bounds[0], geometry.bounds[1]), |m| {
                self.meshes[m].bounds()
            });
            let layers = scene
                .materials
                .get(instance.material)
                .filter(|m| m.extras.terrain_layers > 0)
                .map(|m| Layers {
                    albedo: m
                        .extras
                        .layer_materials
                        .map(|id| shading.get(id as usize).map_or(Vec3::ZERO, |s| s.albedo)),
                    count: m.extras.terrain_layers as usize,
                });
            let mut placed = Placed {
                entity: instance.entity,
                key: geometry.key,
                mesh: mesh.unwrap_or(0),
                world: Mat4::IDENTITY,
                inverse: Mat4::IDENTITY,
                normal: Mat3::IDENTITY,
                shading: shading.get(instance.material).copied().unwrap_or_default(),
                layers,
                local,
                bounds: EMPTY,
                deforming: geometry.deforming || mesh.is_none(),
                included: false,
            };
            placed.place(instance.world);
            placed.included = placed.eligible(dynamic);
            let unchanged = previous.remove(&instance.entity).is_some_and(|old| {
                let same = old.included == placed.included
                    && (!placed.included
                        || (old.world == placed.world
                            && old.key == placed.key
                            && old.shading == placed.shading
                            && old.layers == placed.layers));
                if !same && old.included {
                    changed.push(old.bounds);
                }
                same
            });
            if !unchanged && placed.included {
                changed.push(placed.bounds);
            }
            self.placed.push(placed);
        }
        changed.extend(previous.values().filter(|p| p.included).map(|p| p.bounds));
        self.rebuild_top();
        changed
    }

    /// Moves instances (`worlds`, by entity) and replaces the dynamic set within
    /// one content generation. Returns the world bounds whose traced contents
    /// changed.
    pub(crate) fn edit(
        &mut self,
        worlds: &[(u64, Mat4)],
        dynamic: &BTreeSet<u64>,
    ) -> Vec<(Vec3, Vec3)> {
        let index: HashMap<u64, usize> = self
            .placed
            .iter()
            .enumerate()
            .map(|(i, p)| (p.entity, i))
            .collect();
        let mut changed = Vec::new();
        let mut moved = vec![false; self.placed.len()];
        for &(entity, world) in worlds {
            let Some(&i) = index.get(&entity) else {
                continue;
            };
            let placed = &mut self.placed[i];
            if placed.world == world {
                continue;
            }
            // The old place loses what was traced there.
            if placed.included {
                changed.push(placed.bounds);
            }
            placed.place(world);
            moved[i] = true;
        }
        for (placed, moved) in self.placed.iter_mut().zip(moved) {
            let included = placed.eligible(dynamic);
            // The current place gains (or loses) the instance.
            if (included && (moved || !placed.included)) || (!included && placed.included && !moved)
            {
                changed.push(placed.bounds);
            }
            placed.included = included;
        }
        self.rebuild_top();
        changed
    }

    fn rebuild_top(&mut self) {
        let mut order: Vec<u32> = (0..self.placed.len() as u32)
            .filter(|&i| self.placed[i as usize].included)
            .collect();
        let placed = &self.placed;
        self.top = build(&mut order, |&i| placed[i as usize].bounds, TOP_LEAF);
        self.order = order;
    }

    /// World bounds of the traced instances.
    pub(crate) fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.top.first().map(|n| (n.min, n.max))
    }

    /// The region of built-up content: traced instances that are neither terrain
    /// nor landform-sized, measured as instance volume per [`CELL`] ground cell;
    /// the box of those touching a cell with at least [`DENSITY`] of the densest
    /// cell's volume, grown by [`MARGIN`]. `None` without such instances.
    pub(crate) fn built_up(&self) -> Option<(Vec3, Vec3)> {
        let candidates: Vec<&Placed> = self
            .order
            .iter()
            .map(|&i| &self.placed[i as usize])
            .filter(|p| {
                p.layers.is_none() && (p.bounds.1 - p.bounds.0).length() * 0.5 <= LANDFORM_RADIUS
            })
            .collect();
        let cell = |v: f32| (v / CELL).floor() as i32;
        let cells = |p: &Placed| {
            let (lo, hi) = p.bounds;
            let (x, z) = (cell(lo.x)..=cell(hi.x), cell(lo.z)..=cell(hi.z));
            x.flat_map(move |x| z.clone().map(move |z| (x, z)))
        };
        let mut density: HashMap<(i32, i32), f32> = HashMap::new();
        for p in &candidates {
            let volume = (p.bounds.1 - p.bounds.0 + Vec3::splat(0.25)).element_product();
            let share = volume / cells(p).count() as f32;
            for c in cells(p) {
                *density.entry(c).or_default() += share;
            }
        }
        let threshold = density.values().copied().fold(0., f32::max) * DENSITY;
        candidates
            .iter()
            .filter(|p| cells(p).any(|c| density.get(&c).is_some_and(|&d| d >= threshold)))
            .map(|p| p.bounds)
            .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
            .filter(|_| threshold > 0.)
            .map(|(lo, hi)| (lo - Vec3::splat(MARGIN), hi + Vec3::splat(MARGIN)))
    }

    /// Closest hit along `origin + t * direction` (unit `direction`), 0.001 < t < `maximum`.
    fn trace(&self, origin: Vec3, direction: Vec3, maximum: f32) -> Option<Hit> {
        let seed = [origin, direction]
            .iter()
            .flat_map(|v| v.to_array())
            .fold(0x811c_9dc5u32, |h, v| {
                (h ^ v.to_bits()).wrapping_mul(0x0100_0193)
            });
        let mut hit = None;
        let distance = traverse(
            &self.top,
            origin,
            direction,
            maximum,
            |first, count, limit| {
                let mut closest = None;
                let mut limit = limit;
                for &i in &self.order[first as usize..(first + count) as usize] {
                    let placed = &self.placed[i as usize];
                    let (o, d) = (
                        placed.inverse.transform_point3(origin),
                        placed.inverse.transform_vector3(direction),
                    );
                    let mesh = &self.meshes[placed.mesh];
                    if let Some((t, n, w)) =
                        mesh.trace_cut(o, d, limit, placed.shading.cutout, seed)
                    {
                        limit = t;
                        closest = Some(t);
                        hit = Some((placed, n, w));
                    }
                }
                closest
            },
        );
        hit.map(|(placed, n, w)| Hit {
            distance,
            normal: (placed.normal * n).normalize_or_zero(),
            shading: Shading {
                albedo: placed.layers.map_or(placed.shading.albedo, |l| l.albedo(w)),
                ..placed.shading
            },
        })
    }
}

/// Triangles the CPU bake traces at most: the finest LOD level whose scene total
/// fits is used for every instance (coarser levels where geometry has fewer).
const TRIANGLE_BUDGET: usize = 4_000_000;

/// The finest LOD level index whose total triangle count fits the budget.
fn lod_level(scene: &Scene) -> usize {
    let levels = scene
        .geometry
        .iter()
        .map(|g| g.lods.len())
        .max()
        .unwrap_or(1);
    (0..levels)
        .find(|&level| {
            scene
                .instances
                .iter()
                .filter_map(|i| scene.geometry.get(i.geometry))
                .filter_map(|g| g.lods.get(level.min(g.lods.len().saturating_sub(1))))
                .map(|lod| lod.indices.len() / 3)
                .sum::<usize>()
                <= TRIANGLE_BUDGET
        })
        .unwrap_or(levels.saturating_sub(1))
}

/// Probes of the lattice over every static triangle: alone, or as the coarse
/// lattice of a pool, unchanged, so a pool only ever adds detail.
const PROBES: usize = 32_768;
/// Probes of the fine lattice over built-up content.
const FINE_PROBES: usize = 16_384;
/// Instances wider than this bound radius (m) are landforms, not built-up content.
const LANDFORM_RADIUS: f32 = 32.;
/// Ground cells (m) in which built-up content is measured.
const CELL: f32 = 8.;
/// Cells holding at least this share of the densest cell's instance volume are
/// built up (scattered foliage and props stay well below it).
const DENSITY: f32 = 0.1;
/// Margin (m) around built-up content: the fine lattice fades out beyond it.
const MARGIN: f32 = 6.;
/// A fine lattice is kept only when its spacing is at most this share of the
/// single lattice's.
const FINER: f32 = 0.5;

/// The lattice rule's identity, part of the probe cache's file names
/// (`gi.rs`): any change to the constants above misses older bakes.
pub(crate) fn lattice_rule() -> String {
    format!(
        "pool-p{PROBES}-f{FINE_PROBES}-l{LANDFORM_RADIUS}-g{CELL}-d{DENSITY}-m{MARGIN}-r{FINER}"
    )
}

fn volume(lattice: &ProbeLattice) -> ProbeVolume {
    ProbeVolume {
        origin: Vec3::from_array(lattice.origin),
        spacing: Vec3::from_array(lattice.spacing),
        counts: lattice.counts,
    }
}

fn lattice(volume: &ProbeVolume) -> ProbeLattice {
    ProbeLattice {
        origin: volume.origin.to_array(),
        spacing: volume.spacing.to_array(),
        counts: volume.counts,
    }
}

/// The lattice holding global probe `id` of `layout`, and the id within it.
fn locate(layout: &ProbeLayout, id: usize) -> Option<(ProbeVolume, usize)> {
    layout
        .lattices()
        .iter()
        .zip(layout.bases())
        .find(|(l, base)| id >= *base && id < base + l.probe_count())
        .map(|(l, base)| (volume(l), id - base))
}

/// The probe pool for `tracer`'s static instances, finest first: one lattice
/// over every static triangle, and, where built-up content sits in a markedly
/// smaller region, a fine lattice over it in front of that one.
pub(crate) fn layout(tracer: &Tracer) -> Result<ProbeLayout> {
    let (lo, hi) = tracer
        .bounds()
        .context("the scene has no static triangles to bake")?;
    let single = ProbeVolume::automatic_within(lo, hi, PROBES)?;
    if let Some((flo, fhi)) = tracer.built_up() {
        let fine = ProbeVolume::automatic_within(flo.max(lo), fhi.min(hi), FINE_PROBES)?;
        if fine.spacing.max_element() <= single.spacing.max_element() * FINER {
            return ProbeLayout::new(&[lattice(&fine), lattice(&single)]);
        }
    }
    Ok(ProbeLayout::single(lattice(&single)))
}

/// Lattice position of probe `id` (before relocation).
fn centre(volume: &ProbeVolume, id: usize) -> Vec3 {
    let [cx, cy] = [volume.counts[0] as usize, volume.counts[1] as usize];
    let index = Vec3::new(
        (id % cx) as f32,
        ((id / cx) % cy) as f32,
        (id / (cx * cy)) as f32,
    );
    volume.origin + index * volume.spacing
}

/// Bakes every probe of the tracer's automatic pool ([`layout`]).
#[cfg(test)]
pub(crate) fn bake(tracer: &Tracer) -> Result<ProbeSurfaces> {
    bake_with_layout(tracer, |_| {}, |_| {})
}

/// Publish the final pool's dimensions once, before tracing or allocating the
/// payload: startup reserves its GPU resources while acquisition continues.
/// The coarsest lattice (the old single lattice, all a volume needs to be ready)
/// bakes first and is handed to `coarse` before the finer lattices bake; with a
/// single lattice `coarse` is not called.
pub(crate) fn bake_with_layout(
    tracer: &Tracer,
    publish: impl FnOnce(ProbeLayout),
    coarse: impl FnOnce(&ProbeSurfaces),
) -> Result<ProbeSurfaces> {
    let layout = layout(tracer)?;
    publish(layout);
    let n = layout.lattices().len();
    let mut surfaces = bake_lattices(tracer, layout, n - 1..n)?;
    if n > 1 {
        coarse(&surfaces);
        bake_into(tracer, &mut surfaces, 0..n - 1)?;
    }
    Ok(surfaces)
}

/// Bakes the lattices `which` of `layout` (the rest stay empty).
pub(crate) fn bake_lattices(
    tracer: &Tracer,
    layout: ProbeLayout,
    which: std::ops::Range<usize>,
) -> Result<ProbeSurfaces> {
    let probes = layout.probe_count();
    let mut surfaces = ProbeSurfaces {
        layout,
        texels: vec![[0u32; 4]; probes * SURFACE_TEXELS],
        statics: vec![[0f32; 4]; probes * STATIC_VECTORS],
    };
    bake_into(tracer, &mut surfaces, which)?;
    Ok(surfaces)
}

/// Bakes the lattices `which` of `surfaces`' pool in place.
fn bake_into(
    tracer: &Tracer,
    surfaces: &mut ProbeSurfaces,
    which: std::ops::Range<usize>,
) -> Result<()> {
    let started = std::time::Instant::now();
    let layout = surfaces.layout;
    let bases = layout.bases();
    for i in which.clone() {
        let (lattice, base) = (layout.lattices()[i], bases[i]);
        let volume = volume(&lattice);
        let count = lattice.probe_count();
        let texels = &mut surfaces.texels[base * SURFACE_TEXELS..(base + count) * SURFACE_TEXELS];
        let statics = &mut surfaces.statics[base * STATIC_VECTORS..(base + count) * STATIC_VECTORS];
        // Workers take batches as they free up: buried probes skip their texel
        // rays, so equal shares of probes are far from equal work.
        let batches = std::sync::Mutex::new(
            texels
                .chunks_mut(BATCH * SURFACE_TEXELS)
                .zip(statics.chunks_mut(BATCH * STATIC_VECTORS))
                .zip((0..).step_by(BATCH)),
        );
        std::thread::scope(|scope| {
            for _ in 0..worker_threads() {
                let (batches, volume) = (&batches, &volume);
                scope.spawn(move || {
                    // A poisoned queue means a worker panicked; the scope re-raises it.
                    while let Some(((texels, statics), first)) =
                        batches.lock().ok().and_then(|mut b| b.next())
                    {
                        for local in 0..texels.len() / SURFACE_TEXELS {
                            probe(
                                tracer,
                                volume,
                                first + local,
                                &mut texels[local * SURFACE_TEXELS..(local + 1) * SURFACE_TEXELS],
                                &mut statics[local * STATIC_VECTORS..(local + 1) * STATIC_VECTORS],
                            );
                        }
                    }
                });
            }
        });
    }
    let traced: Vec<_> = layout.lattices()[which].iter().map(|l| l.counts).collect();
    eprintln!(
        "ara-runtime-render: CPU-baked {} GI probes ({traced:?}) in {:.1} s",
        traced
            .iter()
            .map(|c| c.iter().product::<u32>())
            .sum::<u32>(),
        started.elapsed().as_secs_f64()
    );
    surfaces.validate()
}

/// Re-traces `probes` (global ids) of `surfaces` against `tracer`, keeping the pool.
pub(crate) fn rebake(
    tracer: &Tracer,
    surfaces: &mut ProbeSurfaces,
    probes: &[usize],
) -> Result<()> {
    let layout = surfaces.layout;
    ensure!(
        probes.iter().all(|&id| id < layout.probe_count()),
        "probe outside the lattice"
    );
    let threads = worker_threads();
    let chunk = probes.len().div_ceil(threads).max(1);
    let traced = std::thread::scope(|scope| {
        let jobs: Vec<_> = probes
            .chunks(chunk)
            .map(|ids| {
                let layout = &layout;
                scope.spawn(move || {
                    ids.iter()
                        .map(|&id| {
                            let mut texels = [[0u32; 4]; SURFACE_TEXELS];
                            let mut statics = [[0f32; 4]; STATIC_VECTORS];
                            if let Some((volume, local)) = locate(layout, id) {
                                probe(tracer, &volume, local, &mut texels, &mut statics);
                            }
                            (id, texels, statics)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        jobs.into_iter()
            .map(|job| {
                job.join()
                    .map_err(|_| anyhow::anyhow!("a probe trace panicked"))
            })
            .collect::<Result<Vec<_>>>()
    })?;
    for (id, texels, statics) in traced.into_iter().flatten() {
        surfaces.texels[id * SURFACE_TEXELS..(id + 1) * SURFACE_TEXELS].copy_from_slice(&texels);
        surfaces.statics[id * STATIC_VECTORS..(id + 1) * STATIC_VECTORS].copy_from_slice(&statics);
    }
    Ok(())
}

/// Whether the pool of `surfaces` (its coarsest lattice) still encloses `bounds`
/// to within a cell.
pub(crate) fn covers(surfaces: &ProbeSurfaces, (lo, hi): (Vec3, Vec3)) -> bool {
    let Some(coarsest) = surfaces.layout.lattices().last() else {
        return false;
    };
    let volume = volume(coarsest);
    let last = Vec3::from_array(volume.counts.map(|c| c.saturating_sub(1) as f32));
    let end = volume.origin + last * volume.spacing;
    lo.cmpge(volume.origin - volume.spacing).all() && hi.cmple(end + volume.spacing).all()
}

/// Probes an edit inside `boxes` can change: those within one and a half lattice
/// spacings of a box (every cell it touches), or within one and a quarter box
/// diagonals (where the box still covers about 4% of a probe's sphere; beyond,
/// its share of the irradiance is negligible).
pub(crate) fn affected(surfaces: &ProbeSurfaces, boxes: &[(Vec3, Vec3)]) -> Vec<usize> {
    let layout = surfaces.layout;
    let mut ids = Vec::new();
    for (lattice, base) in layout.lattices().iter().zip(layout.bases()) {
        let volume = volume(lattice);
        let cell = volume.spacing.max_element() * 1.5;
        let reach: Vec<_> = boxes
            .iter()
            .filter(|(lo, hi)| lo.cmple(*hi).all())
            .map(|&(lo, hi)| (lo, hi, cell.max((hi - lo).length() * 1.25)))
            .collect();
        ids.extend((0..volume.len()).filter_map(|id| {
            let c = centre(&volume, id);
            reach
                .iter()
                .any(|&(lo, hi, r)| (lo - c).max(c - hi).max(Vec3::ZERO).length() < r)
                .then_some(base + id)
        }));
    }
    ids
}

fn oct_encode(n: Vec3) -> Vec2 {
    let n = n / (n.x.abs() + n.y.abs() + n.z.abs());
    let mut p = Vec2::new(n.x, n.y);
    if n.z < 0. {
        let s = Vec2::new(
            if p.x >= 0. { 1. } else { -1. },
            if p.y >= 0. { 1. } else { -1. },
        );
        p = (Vec2::ONE - Vec2::new(p.y.abs(), p.x.abs())) * s;
    }
    p * 0.5 + 0.5
}

fn oct_decode(p: Vec2) -> Vec3 {
    let p = p * 2. - 1.;
    let mut n = Vec3::new(p.x, p.y, 1. - p.x.abs() - p.y.abs());
    let t = (-n.z).clamp(0., 1.);
    n.x += if n.x >= 0. { -t } else { t };
    n.y += if n.y >= 0. { -t } else { t };
    n.normalize()
}

fn unorm8(v: f32) -> u32 {
    (v.clamp(0., 1.) * 255.).round() as u32
}

/// Nonnegative float to IEEE half bits (truncating, flushing denormals, clamping
/// to the largest finite half), as the GPU bake packs them.
fn half_bits(v: f32) -> u32 {
    let x = v.max(0.).to_bits();
    let e = ((x >> 23) & 255) as i32 - 112;
    if e <= 0 {
        0
    } else if e >= 31 {
        0x7bff
    } else {
        ((e as u32) << 10) | ((x & 0x7fffff) >> 13)
    }
}

/// One probe: relocation and classification, then the 8x8 cache texels (sixteen
/// stratified rays each) and distance moments, exactly as `probeSurfacesMain`.
fn probe(
    tracer: &Tracer,
    volume: &ProbeVolume,
    id: usize,
    texels: &mut [[u32; 4]],
    statics: &mut [[f32; 4]],
) {
    let centre = centre(volume, id);
    let fibonacci = |lane: usize| {
        let y = 1. - 2. * (lane as f32 + 0.5) / 64.;
        let phi = lane as f32 * 2.399_963_2;
        let r = (1. - y * y).max(0.).sqrt();
        Vec3::new(phi.cos() * r, y, phi.sin() * r)
    };
    let back = |origin: Vec3, d: Vec3| {
        tracer
            .trace(origin, d, RANGE)
            .map(|h| (h.distance, h.normal.dot(d) > 0.))
    };
    let (mut count, mut nearest, mut nearest_back) =
        (0usize, (RANGE, Vec3::ZERO), (RANGE, Vec3::ZERO));
    for lane in 0..64 {
        let d = fibonacci(lane);
        if let Some((t, is_back)) = back(centre, d) {
            if is_back {
                count += 1;
                if t < nearest_back.0 {
                    nearest_back = (t, d);
                }
            }
            if t < nearest.0 {
                nearest = (t, d);
            }
        }
    }
    let mut relocation = Vec3::ZERO;
    if count > 16 {
        relocation = nearest_back.1 * (volume.spacing.length() * 0.25).min(nearest_back.0 + 0.03);
    } else if nearest.0 < 0.12 {
        relocation = -nearest.1 * (0.12 - nearest.0);
    }
    let offset = relocation.clamp(-volume.spacing * 0.45, volume.spacing * 0.45);
    let origin = centre + offset;
    let backs = (0..64)
        .filter(|&lane| back(origin, fibonacci(lane)).is_some_and(|(_, b)| b))
        .count();
    let active = backs <= 16;
    statics[64] = [offset.x, offset.y, offset.z, if active { 1. } else { 0. }];
    statics[65] = [backs as f32, 0., 0., 0.];
    // A buried probe stays dark and is never read (relight skips it, shading
    // weights it out), so its texel rays are not traced: its texels stay zero.
    if !active {
        return;
    }
    for lane in 0..64 {
        let (mut albedo, mut emissive, mut normal) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        let (mut centroid, mut moment1, mut moment2, mut geometry) = (Vec3::ZERO, 0f32, 0f32, 0u32);
        for k in 0..16 {
            let uv = (Vec2::new((lane % 8) as f32, (lane / 8) as f32)
                + (Vec2::new((k % 4) as f32, (k / 4) as f32) + 0.5) / 4.)
                / 8.;
            let dir = oct_decode(uv);
            let mut t = RANGE;
            if let Some(hit) = tracer.trace(origin, dir, RANGE) {
                t = hit.distance;
                geometry += 1;
                centroid += dir * t;
                if hit.normal.dot(dir) < 0. {
                    albedo += hit.shading.albedo;
                    emissive += hit.shading.emissive;
                    normal += hit.normal;
                } else {
                    normal -= dir;
                }
            }
            moment1 += t;
            moment2 += t * t;
        }
        let sky = 1. - geometry as f32 / 16.;
        let g = geometry.max(1) as f32;
        let (albedo, emissive, centroid) = (albedo / g, emissive / g, centroid / g);
        let normal = if normal.length_squared() > 0. {
            normal.normalize()
        } else {
            -oct_decode((Vec2::new((lane % 8) as f32, (lane / 8) as f32) + 0.5) / 8.)
        };
        // The centre ray's distance to the plane through the hits' centroid along their
        // mean normal, as `probeSurfacesMain`: relighting rebuilds one hit per texel on
        // the centre ray, and the hits' mean distance would land behind a surface seen at
        // an angle (distance grows as 1/cos across the texel), shadowing itself.
        let axis = oct_decode((Vec2::new((lane % 8) as f32, (lane / 8) as f32) + 0.5) / 8.);
        let facing = normal.dot(axis);
        let mean = if facing < -0.05 {
            (normal.dot(centroid) / facing).clamp(0., RANGE)
        } else {
            centroid.length()
        };
        let n = oct_encode(normal);
        texels[lane] = [
            unorm8(albedo.x) | unorm8(albedo.y) << 8 | unorm8(albedo.z) << 16 | unorm8(sky) << 24,
            (n.x * 65535.).round() as u32 | ((n.y * 65535.).round() as u32) << 16,
            half_bits(emissive.x) | half_bits(emissive.y) << 16,
            half_bits(emissive.z) | half_bits(mean) << 16,
        ];
        statics[lane] = [moment1 / 16., moment2 / 16., 0., 0.];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_render::instanced::{Instance, Lod, NO_TEXTURE, Vertex};

    #[test]
    fn allocation_layout_precedes_a_blocked_bake_and_matches_the_delivered_payload() -> Result<()> {
        let floor = [
            Vec3::new(-1., 0., -1.),
            Vec3::new(-1., 0., 1.),
            Vec3::new(1., 0., 1.),
            Vec3::new(1., 0., -1.),
        ];
        let source = scene(vec![geometry(1, &[floor], Vec3::Y)]);
        let tracer = Tracer::new(&source, &[Shading::default()], &BTreeSet::new());
        let (send, receive) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            bake_with_layout(
                &tracer,
                |layout| {
                    send.send(layout)
                        .expect("layout receiver lives through the bake");
                    wait.recv().expect("test releases the bake");
                },
                |_| {},
            )
        });
        let layout = receive.recv_timeout(std::time::Duration::from_secs(5))?;
        layout.validate()?;
        assert!(
            !worker.is_finished(),
            "GPU reservation can proceed before the payload exists"
        );
        release.send(())?;
        let surfaces = worker
            .join()
            .map_err(|_| anyhow::anyhow!("probe bake panicked"))??;
        assert_eq!(surfaces.layout(), layout);
        Ok(())
    }

    #[test]
    fn a_pool_keeps_the_single_lattice_and_adds_a_fine_one_over_built_up_content() -> Result<()> {
        // A 400 m landform with a 4 m wall at its centre.
        let ground = [
            Vec3::new(-200., 0., -200.),
            Vec3::new(-200., 0., 200.),
            Vec3::new(200., 0., 200.),
            Vec3::new(200., 0., -200.),
        ];
        let wall = [
            Vec3::new(-2., 0., 0.),
            Vec3::new(-2., 4., 0.),
            Vec3::new(2., 4., 0.),
            Vec3::new(2., 0., 0.),
        ];
        let single = |tracer: &Tracer| -> Result<ProbeLattice> {
            let (lo, hi) = tracer.bounds().context("static triangles")?;
            Ok(lattice(&ProbeVolume::automatic_within(lo, hi, PROBES)?))
        };
        let shading = [Shading::default(); 2];
        let open = scene(vec![geometry(1, &[ground], Vec3::Y)]);
        let tracer = Tracer::new(&open, &shading, &BTreeSet::new());
        assert_eq!(layout(&tracer)?.lattices(), [single(&tracer)?]);
        let built = scene(vec![
            geometry(1, &[ground], Vec3::Y),
            geometry(2, &[wall], Vec3::new(0., 2., 5.)),
        ]);
        let tracer = Tracer::new(&built, &shading, &BTreeSet::new());
        let pool = layout(&tracer)?;
        let [fine, coarse] = pool.lattices() else {
            anyhow::bail!("a fine lattice over the wall: {pool:?}");
        };
        assert_eq!(*coarse, single(&tracer)?, "a pool only ever adds detail");
        assert!(fine.spacing[0] <= coarse.spacing[0] * FINER);
        Ok(())
    }

    /// The quad `q` (two triangles) with normals facing `inside`.
    fn facing(q: [Vec3; 4], inside: Vec3) -> Vec3 {
        let n = (q[1] - q[0]).cross(q[2] - q[0]).normalize();
        let centre = (q[0] + q[1] + q[2] + q[3]) / 4.;
        if n.dot(inside - centre) < 0. { -n } else { n }
    }

    fn geometry(key: u8, quads: &[[Vec3; 4]], inside: Vec3) -> Geometry {
        let (mut vertices, mut indices) = (Vec::new(), Vec::new());
        for &q in quads {
            let base = vertices.len() as u32;
            let n = facing(q, inside);
            vertices.extend(q.map(|p| Vertex {
                position: p.to_array(),
                normal: n.to_array(),
                uv: [0.; 2],
                tangent: [1., 0., 0., 1.],
                uv1: [0.; 2],
                terrain_weights: 0,
                reserved: 0,
            }));
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let (lo, hi) = quads
            .iter()
            .flatten()
            .fold(EMPTY, |acc, &p| union(acc, (p, p)));
        Geometry {
            key: [key; 32],
            vertices,
            lods: vec![Lod { indices, error: 0. }],
            bounds: [lo, hi],
            deforming: false,
            revision: 0,
        }
    }

    fn scene(geometry: Vec<Geometry>) -> Scene {
        let instances = (0..geometry.len())
            .map(|i| Instance {
                entity: i as u64 + 1,
                name: String::new(),
                geometry: i,
                material: i,
                world: Mat4::IDENTITY,
                previous_world: Mat4::IDENTITY,
            })
            .collect();
        Scene {
            geometry,
            instances,
            materials: Vec::new(),
            textures: 0,
            lights: Vec::new(),
            sun_direction: Vec3::Y,
            sun_lux: Vec3::ZERO,
            environment: Vec::new(),
            environment_size: [0, 0],
        }
    }

    #[test]
    fn traversal_finds_the_closest_of_many_triangles() -> Result<()> {
        // A stack of 64 parallel floors; the ray from above hits the topmost.
        let mut triangles = Vec::new();
        for i in 0..64 {
            let y = i as f32 * 0.5;
            let q = [
                Vec3::new(-1., y, -1.),
                Vec3::new(-1., y, 1.),
                Vec3::new(1., y, 1.),
                Vec3::new(1., y, -1.),
            ];
            let normal = facing(q, Vec3::new(0., 1000., 0.));
            for (b, c) in [(q[1], q[2]), (q[2], q[3])] {
                triangles.push(Triangle {
                    a: q[0],
                    e1: b - q[0],
                    e2: c - q[0],
                    normal,
                    weights: [0; 4],
                });
            }
        }
        let mesh = Mesh::new(triangles);
        assert!(mesh.nodes.len() > 1, "the BVH split");
        let (distance, normal, _) = mesh
            .trace(Vec3::new(0.2, 100., 0.1), Vec3::NEG_Y, RANGE)
            .context("the ray hits the top floor")?;
        assert!((distance - (100. - 31.5)).abs() < 1e-3);
        assert!(normal.y > 0.99);
        assert!(
            mesh.trace(Vec3::new(5., 100., 5.), Vec3::NEG_Y, RANGE)
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn terrain_hits_blend_their_layers_by_splat_weight() -> Result<()> {
        let floor = [
            Vec3::new(-1., 0., -1.),
            Vec3::new(-1., 0., 1.),
            Vec3::new(1., 0., 1.),
            Vec3::new(1., 0., -1.),
        ];
        let mut ground = geometry(1, &[floor], Vec3::Y);
        // A quarter layer 0, three quarters layer 1.
        for v in &mut ground.vertices {
            v.terrain_weights = u32::from_le_bytes([64, 192, 0, 0]);
        }
        let mut scene = scene(vec![ground]);
        let mut terrain = ara_render::instanced::Material::default();
        terrain.extras.terrain_layers = 2;
        terrain.extras.layer_materials = [1, 2, NO_TEXTURE, NO_TEXTURE];
        scene.materials = vec![terrain, Default::default(), Default::default()];
        let shading = [Vec3::splat(0.5), Vec3::X, Vec3::Y].map(|albedo| Shading {
            albedo,
            ..Shading::default()
        });
        let albedo = |scene: &Scene| {
            Tracer::new(scene, &shading, &BTreeSet::new())
                .trace(Vec3::new(0.2, 5., 0.1), Vec3::NEG_Y, RANGE)
                .map(|hit| hit.shading.albedo)
                .context("the ground")
        };
        let blended = albedo(&scene)?;
        assert!(
            (blended - Vec3::new(0.25, 0.75, 0.)).length() < 0.01,
            "{blended}"
        );
        // A slot past `terrain_layers` weighs nothing: layer 0 alone remains.
        scene.materials[0].extras.terrain_layers = 1;
        assert_eq!(albedo(&scene)?, Vec3::X);
        Ok(())
    }

    #[test]
    fn rays_pass_cut_out_surfaces_by_their_share() {
        // A floor and, 1 m above it, a canopy letting through `cutout` of the rays.
        let quad = |y: f32| {
            [
                Vec3::new(-1., y, -1.),
                Vec3::new(-1., y, 1.),
                Vec3::new(1., y, 1.),
                Vec3::new(1., y, -1.),
            ]
        };
        let scene = scene(vec![
            geometry(1, &[quad(0.)], Vec3::Y),
            geometry(2, &[quad(1.)], Vec3::NEG_Y),
        ]);
        let through = |cutout: f32| {
            let canopy = Shading {
                cutout,
                ..Shading::default()
            };
            let tracer = Tracer::new(&scene, &[Shading::default(), canopy], &BTreeSet::new());
            (0..1000)
                .filter(|&i| {
                    let at = Vec3::new(i as f32 / 1000. * 1.6 - 0.8, 5., (i % 7) as f32 * 0.1);
                    tracer
                        .trace(at, Vec3::NEG_Y, RANGE)
                        .is_some_and(|hit| (hit.distance - 5.).abs() < 1e-3)
                })
                .count()
        };
        assert_eq!(through(0.), 0, "an opaque canopy stops every ray");
        assert_eq!(through(1.), 1000);
        let half = through(0.5);
        assert!((400..600).contains(&half), "{half}");
    }

    #[test]
    fn instances_trace_through_their_transforms() -> Result<()> {
        // One unit floor quad, placed twice: raised 2 m and scaled 3x.
        let floor = [
            Vec3::new(-0.5, 0., -0.5),
            Vec3::new(-0.5, 0., 0.5),
            Vec3::new(0.5, 0., 0.5),
            Vec3::new(0.5, 0., -0.5),
        ];
        let mut scene = scene(vec![geometry(1, &[floor], Vec3::Y)]);
        scene.instances[0].world = Mat4::from_translation(Vec3::new(0., 2., 0.));
        let shading = [Shading {
            albedo: Vec3::splat(0.5),
            ..Shading::default()
        }];
        let mut tracer = Tracer::new(&scene, &shading, &BTreeSet::new());
        let hit = tracer
            .trace(Vec3::new(0.2, 10., 0.), Vec3::NEG_Y, RANGE)
            .context("the raised floor")?;
        assert!((hit.distance - 8.).abs() < 1e-4 && hit.normal.y > 0.99);
        let scaled = Mat4::from_scale(Vec3::splat(3.));
        let changed = tracer.edit(&[(1, scaled)], &BTreeSet::new());
        assert_eq!(changed.len(), 2, "old and new places");
        let hit = tracer
            .trace(Vec3::new(1.4, 10., 0.), Vec3::NEG_Y, RANGE)
            .context("the scaled floor")?;
        assert!((hit.distance - 10.).abs() < 1e-4);
        assert_eq!(tracer.edit(&[], &BTreeSet::from([1])).len(), 1);
        assert!(
            tracer
                .trace(Vec3::new(1.4, 10., 0.), Vec3::NEG_Y, RANGE)
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn oct_mapping_and_halves_match_the_shader() {
        for d in [Vec3::X, Vec3::NEG_Y, Vec3::new(0.3, -0.5, -0.8).normalize()] {
            assert!((oct_decode(oct_encode(d)) - d).length() < 1e-4);
        }
        assert_eq!(half_bits(1.), 0x3c00);
        assert_eq!(half_bits(1e9), 0x7bff);
    }

    #[test]
    fn a_probe_inside_a_closed_emissive_room_sees_its_walls() {
        // A 4 m cube, faces pointing inward; the ceiling emits.
        let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z) * 2.;
        let (l, h) = (-1., 1.);
        let walls = [
            [c(l, l, l), c(h, l, l), c(h, l, h), c(l, l, h)],
            [c(l, l, l), c(l, l, h), c(l, h, h), c(l, h, l)],
            [c(h, l, l), c(h, h, l), c(h, h, h), c(h, l, h)],
            [c(l, l, l), c(l, h, l), c(h, h, l), c(h, l, l)],
            [c(l, l, h), c(h, l, h), c(h, h, h), c(l, h, h)],
        ];
        let ceiling = [[c(l, h, l), c(l, h, h), c(h, h, h), c(h, h, l)]];
        let scene = scene(vec![
            geometry(1, &walls, Vec3::ZERO),
            geometry(2, &ceiling, Vec3::ZERO),
        ]);
        let shading = [
            Shading {
                albedo: Vec3::splat(0.5),
                ..Shading::default()
            },
            Shading {
                emissive: Vec3::splat(4.),
                ..Shading::default()
            },
        ];
        let tracer = Tracer::new(&scene, &shading, &BTreeSet::new());
        let volume = ProbeVolume {
            origin: Vec3::new(-0.5, -0.5, -0.5),
            spacing: Vec3::ONE,
            counts: [2, 2, 2],
        };
        let mut texels = vec![[0u32; 4]; SURFACE_TEXELS];
        let mut statics = vec![[0f32; 4]; STATIC_VECTORS];
        probe(&tracer, &volume, 0, &mut texels, &mut statics);
        assert_eq!(statics[64][3], 1., "an open probe is active");
        // Every texel hits a wall (no sky); some see the emissive ceiling.
        assert!(texels.iter().all(|t| t[0] >> 24 == 0));
        let emitting = texels.iter().filter(|t| (t[2] & 0xffff) != 0).count();
        assert!(emitting > 0 && emitting < SURFACE_TEXELS, "{emitting}");
    }
}
