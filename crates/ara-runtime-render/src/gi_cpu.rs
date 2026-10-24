//! CPU probe-surface acquisition for devices without hardware ray queries: a
//! binned-SAH BVH over the native scene (the finest LOD level within a triangle
//! budget), traced in
//! parallel with the rays, relocation and packing of the GPU bake
//! (`probe-surfaces.slang`), so every GPU gets the same probe GI. Materials use
//! their factors times each map's mean texel; alpha-tested surfaces count as opaque.
use anyhow::{Result, ensure};
use ara_render::{
    instanced::Scene,
    look::gi::{ProbeSurfaces, STATIC_VECTORS, SURFACE_TEXELS},
};
use ara_render_fidelity::probes::ProbeVolume;
use glam30::{Vec2, Vec3};

/// Longest probe ray, matching the GPU bake.
const RANGE: f32 = 200.;
/// Triangles per leaf.
const LEAF: usize = 4;
const BINS: usize = 16;

/// Diffuse albedo (metal removed), emissive radiance.
#[derive(Clone, Copy, Default)]
pub(crate) struct Shading {
    pub albedo: Vec3,
    pub emissive: Vec3,
}

#[derive(Clone, Copy)]
struct Triangle {
    a: Vec3,
    e1: Vec3,
    e2: Vec3,
    /// Geometric normal oriented with the vertex normals.
    normal: Vec3,
    material: u32,
}

#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: first triangle; interior: left child (right is `first + 1`).
    first: u32,
    /// Leaf triangle count; zero for interior nodes.
    count: u32,
}

/// A bounding volume hierarchy over world-space triangles.
struct Bvh {
    nodes: Vec<Node>,
    triangles: Vec<Triangle>,
}

struct Hit {
    distance: f32,
    triangle: u32,
}

fn bounds(triangles: &[Triangle]) -> (Vec3, Vec3) {
    triangles.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(lo, hi), t| {
            let (b, c) = (t.a + t.e1, t.a + t.e2);
            (lo.min(t.a).min(b).min(c), hi.max(t.a).max(b).max(c))
        },
    )
}

fn centroid(t: &Triangle) -> Vec3 {
    t.a + (t.e1 + t.e2) / 3.
}

fn area(lo: Vec3, hi: Vec3) -> f32 {
    let d = (hi - lo).max(Vec3::ZERO);
    d.x * d.y + d.y * d.z + d.z * d.x
}

impl Bvh {
    /// Binned surface-area-heuristic build over centroids.
    fn build(mut triangles: Vec<Triangle>) -> Self {
        let mut nodes = Vec::with_capacity(triangles.len() / LEAF * 2 + 1);
        let (lo, hi) = bounds(&triangles);
        nodes.push(Node {
            min: lo,
            max: hi,
            first: 0,
            count: triangles.len() as u32,
        });
        let mut stack = vec![0usize];
        while let Some(index) = stack.pop() {
            let node = nodes[index];
            let (start, count) = (node.first as usize, node.count as usize);
            if count <= LEAF {
                continue;
            }
            let span = &mut triangles[start..start + count];
            let (clo, chi) = span.iter().fold(
                (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                |(lo, hi), t| (lo.min(centroid(t)), hi.max(centroid(t))),
            );
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
            let bin = |t: &Triangle| {
                (((centroid(t)[axis] - clo[axis]) / extent[axis] * BINS as f32) as usize)
                    .min(BINS - 1)
            };
            let mut bins = [(
                0usize,
                Vec3::splat(f32::INFINITY),
                Vec3::splat(f32::NEG_INFINITY),
            ); BINS];
            for t in span.iter() {
                let b = &mut bins[bin(t)];
                let (tlo, thi) = bounds(std::slice::from_ref(t));
                *b = (b.0 + 1, b.1.min(tlo), b.2.max(thi));
            }
            // Sweep the split planes for the cheapest SAH cost.
            let mut best = (f32::INFINITY, 0usize);
            for split in 1..BINS {
                let side = |range: std::ops::Range<usize>| {
                    bins[range].iter().fold(
                        (
                            0usize,
                            Vec3::splat(f32::INFINITY),
                            Vec3::splat(f32::NEG_INFINITY),
                        ),
                        |(n, lo, hi), b| (n + b.0, lo.min(b.1), hi.max(b.2)),
                    )
                };
                let (ln, llo, lhi) = side(0..split);
                let (rn, rlo, rhi) = side(split..BINS);
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
            // Partition the span by bin.
            let (mut i, mut j) = (0usize, count);
            while i < j {
                if bin(&span[i]) < best.1 {
                    i += 1;
                } else {
                    j -= 1;
                    span.swap(i, j);
                }
            }
            if i == 0 || i == count {
                continue;
            }
            let left = nodes.len();
            for (first, n) in [(start, i), (start + i, count - i)] {
                let (lo, hi) = bounds(&triangles[first..first + n]);
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
        Self { nodes, triangles }
    }

    /// Closest hit along `origin + t * direction`, 0.001 < t < `maximum`.
    fn trace(&self, origin: Vec3, direction: Vec3, maximum: f32) -> Option<Hit> {
        let inverse = direction.recip();
        let mut best = Hit {
            distance: maximum,
            triangle: u32::MAX,
        };
        let slab = |node: &Node, limit: f32| {
            let t0 = (node.min - origin) * inverse;
            let t1 = (node.max - origin) * inverse;
            let near = t0.min(t1).max_element().max(0.);
            let far = t0.max(t1).min_element().min(limit);
            (near <= far).then_some(near)
        };
        let mut stack = [0u32; 64];
        let mut depth = 1usize;
        if self.nodes.is_empty() || slab(&self.nodes[0], maximum).is_none() {
            return None;
        }
        while depth > 0 {
            depth -= 1;
            let node = &self.nodes[stack[depth] as usize];
            if slab(node, best.distance).is_none() {
                continue;
            }
            if node.count > 0 {
                for index in node.first..node.first + node.count {
                    let t = &self.triangles[index as usize];
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
                    let distance = t.e2.dot(q) * inv;
                    if distance > 0.001 && distance < best.distance {
                        best = Hit {
                            distance,
                            triangle: index,
                        };
                    }
                }
                continue;
            }
            // Visit the nearer child first.
            let (l, r) = (node.first, node.first + 1);
            let (dl, dr) = (
                slab(&self.nodes[l as usize], best.distance),
                slab(&self.nodes[r as usize], best.distance),
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
        (best.triangle != u32::MAX).then_some(best)
    }
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

/// The world-space triangles at the budgeted LOD level of every instance.
fn triangles(scene: &Scene) -> Vec<Triangle> {
    let level = lod_level(scene);
    let mut out = Vec::new();
    for instance in &scene.instances {
        let Some(geometry) = scene.geometry.get(instance.geometry) else {
            continue;
        };
        let Some(lod) = geometry
            .lods
            .get(level.min(geometry.lods.len().saturating_sub(1)))
        else {
            continue;
        };
        let world = instance.world;
        let normal_matrix = world.inverse().transpose();
        for tri in lod.indices.as_chunks::<3>().0 {
            let [a, b, c] = tri.map(|i| &geometry.vertices[i as usize]);
            let [pa, pb, pc] =
                [a, b, c].map(|v| world.transform_point3(Vec3::from_array(v.position)));
            let (e1, e2) = (pb - pa, pc - pa);
            let mut normal = e1.cross(e2).normalize_or_zero();
            if normal == Vec3::ZERO {
                continue;
            }
            let shading = normal_matrix.transform_vector3(
                Vec3::from_array(a.normal)
                    + Vec3::from_array(b.normal)
                    + Vec3::from_array(c.normal),
            );
            if normal.dot(shading) < 0. {
                normal = -normal;
            }
            out.push(Triangle {
                a: pa,
                e1,
                e2,
                normal,
                material: instance.material as u32,
            });
        }
    }
    out
}

/// Bakes probe surfaces for `scene` on the CPU. `shading` gives each material's
/// diffuse albedo and emissive radiance.
pub(crate) fn bake(scene: &Scene, shading: &[Shading]) -> Result<ProbeSurfaces> {
    let started = std::time::Instant::now();
    let triangles = triangles(scene);
    ensure!(!triangles.is_empty(), "the scene has no triangles to bake");
    let (lo, hi) = bounds(&triangles);
    let volume = ProbeVolume::automatic(lo, hi)?;
    let bvh = Bvh::build(triangles);
    let probes = volume.len();
    let mut texels = vec![[0u32; 4]; probes * SURFACE_TEXELS];
    let mut statics = vec![[0f32; 4]; probes * STATIC_VECTORS];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = probes.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for ((texels, statics), first) in texels
            .chunks_mut(chunk * SURFACE_TEXELS)
            .zip(statics.chunks_mut(chunk * STATIC_VECTORS))
            .zip((0..).step_by(chunk))
        {
            let (bvh, shading, volume) = (&bvh, shading, &volume);
            scope.spawn(move || {
                for local in 0..texels.len() / SURFACE_TEXELS {
                    probe(
                        bvh,
                        shading,
                        volume,
                        first + local,
                        &mut texels[local * SURFACE_TEXELS..(local + 1) * SURFACE_TEXELS],
                        &mut statics[local * STATIC_VECTORS..(local + 1) * STATIC_VECTORS],
                    );
                }
            });
        }
    });
    eprintln!(
        "ara-runtime-render: CPU-baked {probes} GI probes ({:?}, {} triangles) in {:.1} s",
        volume.counts,
        bvh.triangles.len(),
        started.elapsed().as_secs_f64()
    );
    let surfaces = ProbeSurfaces {
        origin: volume.origin.to_array(),
        spacing: volume.spacing.to_array(),
        counts: volume.counts,
        texels,
        statics,
    };
    surfaces.validate()?;
    Ok(surfaces)
}

/// One probe: relocation and classification, then the 8x8 cache texels (sixteen
/// stratified rays each) and distance moments, exactly as `probeSurfacesMain`.
fn probe(
    bvh: &Bvh,
    shading: &[Shading],
    volume: &ProbeVolume,
    id: usize,
    texels: &mut [[u32; 4]],
    statics: &mut [[f32; 4]],
) {
    let [cx, cy] = [volume.counts[0] as usize, volume.counts[1] as usize];
    let index = Vec3::new(
        (id % cx) as f32,
        ((id / cx) % cy) as f32,
        (id / (cx * cy)) as f32,
    );
    let centre = volume.origin + index * volume.spacing;
    let fibonacci = |lane: usize| {
        let y = 1. - 2. * (lane as f32 + 0.5) / 64.;
        let phi = lane as f32 * 2.399_963_2;
        let r = (1. - y * y).max(0.).sqrt();
        Vec3::new(phi.cos() * r, y, phi.sin() * r)
    };
    let back = |origin: Vec3, d: Vec3| {
        bvh.trace(origin, d, RANGE).map(|h| {
            (
                h.distance,
                bvh.triangles[h.triangle as usize].normal.dot(d) > 0.,
            )
        })
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
    for lane in 0..64 {
        let (mut albedo, mut emissive, mut normal) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        let (mut mean, mut moment1, mut moment2, mut geometry) = (0f32, 0f32, 0f32, 0u32);
        for k in 0..16 {
            let uv = (Vec2::new((lane % 8) as f32, (lane / 8) as f32)
                + (Vec2::new((k % 4) as f32, (k / 4) as f32) + 0.5) / 4.)
                / 8.;
            let dir = oct_decode(uv);
            let mut t = RANGE;
            if let Some(hit) = bvh.trace(origin, dir, RANGE) {
                t = hit.distance;
                geometry += 1;
                mean += t;
                let triangle = &bvh.triangles[hit.triangle as usize];
                if triangle.normal.dot(dir) < 0. {
                    let s = shading
                        .get(triangle.material as usize)
                        .copied()
                        .unwrap_or_default();
                    albedo += s.albedo;
                    emissive += s.emissive;
                    normal += triangle.normal;
                } else {
                    normal -= dir;
                }
            }
            moment1 += t;
            moment2 += t * t;
        }
        let sky = 1. - geometry as f32 / 16.;
        let g = geometry.max(1) as f32;
        let (albedo, emissive, mean) = (albedo / g, emissive / g, mean / g);
        let normal = if normal.length_squared() > 0. {
            normal.normalize()
        } else {
            -oct_decode((Vec2::new((lane % 8) as f32, (lane / 8) as f32) + 0.5) / 8.)
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
    statics[64] = [
        offset.x,
        offset.y,
        offset.z,
        if backs > 16 { 0. } else { 1. },
    ];
    statics[65] = [backs as f32, 0., 0., 0.];
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(a: Vec3, b: Vec3, c: Vec3, d: Vec3, material: u32) -> [Triangle; 2] {
        // Faces point towards the origin (the inside of the test room).
        let tri = |p: Vec3, q: Vec3, r: Vec3| {
            let n = (q - p).cross(r - p).normalize();
            Triangle {
                a: p,
                e1: q - p,
                e2: r - p,
                normal: if n.dot(p + (q - p + r - p) / 3.) > 0. {
                    -n
                } else {
                    n
                },
                material,
            }
        };
        [tri(a, b, c), tri(a, c, d)]
    }

    #[test]
    fn traversal_finds_the_closest_of_many_triangles() -> Result<()> {
        // A stack of 64 parallel floors; the ray from above hits the topmost.
        let mut triangles = Vec::new();
        for i in 0..64 {
            let y = i as f32 * 0.5;
            triangles.extend(quad(
                Vec3::new(-1., y, -1.),
                Vec3::new(-1., y, 1.),
                Vec3::new(1., y, 1.),
                Vec3::new(1., y, -1.),
                i,
            ));
        }
        let bvh = Bvh::build(triangles);
        assert!(bvh.nodes.len() > 1, "the BVH split");
        let hit = bvh
            .trace(Vec3::new(0.2, 100., 0.1), Vec3::NEG_Y, RANGE)
            .ok_or_else(|| anyhow::anyhow!("the ray hits the top floor"))?;
        assert!((hit.distance - (100. - 31.5)).abs() < 1e-3);
        assert_eq!(bvh.triangles[hit.triangle as usize].material, 63);
        assert!(
            bvh.trace(Vec3::new(5., 100., 5.), Vec3::NEG_Y, RANGE)
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
        let corner = |x: f32, y: f32, z: f32| Vec3::new(x, y, z) * 2.;
        let mut triangles = Vec::new();
        let (l, h) = (-1., 1.);
        triangles.extend(quad(
            corner(l, l, l),
            corner(h, l, l),
            corner(h, l, h),
            corner(l, l, h),
            0,
        ));
        triangles.extend(quad(
            corner(l, h, l),
            corner(l, h, h),
            corner(h, h, h),
            corner(h, h, l),
            1,
        ));
        triangles.extend(quad(
            corner(l, l, l),
            corner(l, l, h),
            corner(l, h, h),
            corner(l, h, l),
            0,
        ));
        triangles.extend(quad(
            corner(h, l, l),
            corner(h, h, l),
            corner(h, h, h),
            corner(h, l, h),
            0,
        ));
        triangles.extend(quad(
            corner(l, l, l),
            corner(l, h, l),
            corner(h, h, l),
            corner(h, l, l),
            0,
        ));
        triangles.extend(quad(
            corner(l, l, h),
            corner(h, l, h),
            corner(h, h, h),
            corner(l, h, h),
            0,
        ));
        let bvh = Bvh::build(triangles);
        let shading = [
            Shading {
                albedo: Vec3::splat(0.5),
                emissive: Vec3::ZERO,
            },
            Shading {
                albedo: Vec3::ZERO,
                emissive: Vec3::splat(4.),
            },
        ];
        let volume = ProbeVolume {
            origin: Vec3::new(-0.5, -0.5, -0.5),
            spacing: Vec3::ONE,
            counts: [2, 2, 2],
        };
        let mut texels = vec![[0u32; 4]; SURFACE_TEXELS];
        let mut statics = vec![[0f32; 4]; STATIC_VECTORS];
        probe(&bvh, &shading, &volume, 0, &mut texels, &mut statics);
        assert_eq!(statics[64][3], 1., "an open probe is active");
        // Every texel hits a wall (no sky); some see the emissive ceiling.
        assert!(texels.iter().all(|t| t[0] >> 24 == 0));
        let emitting = texels.iter().filter(|t| (t[2] & 0xffff) != 0).count();
        assert!(emitting > 0 && emitting < SURFACE_TEXELS, "{emitting}");
    }
}
