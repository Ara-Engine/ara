//! `trace()` on the GPU (`shaders/trace-test.slang`) against an f64 brute force over
//! every world-space triangle: closest and any hits, t ranges, masks, face culling,
//! mirrored and non-uniformly scaled instances, alpha-tested submeshes, refits and a
//! republish.
use super::*;
use crate::look::trace::gpu::{SCENE_ALPHA, SCENE_NONEMPTY};
use anyhow::{Result, ensure};
use ara_gpu::vulkan::Command;
use ara_gpu::{
    Access, Barrier, Buffer, Device, GpuDevice, Pipeline, Queue, Resource, RootConstants, vk,
};
use glam::{DAffine3, DVec3, Mat4};

const ACCEPT_FIRST: u32 = 1;
const CULL_BACK: u32 = 2;
const FORCE_OPAQUE: u32 = 8;
/// Checker cells of the alpha texture, in texels (16 x 16 texels).
const CELL: usize = 4;

/// A GPU device with the cooked trace pipelines.
pub(super) struct Harness {
    pub gpu: GpuDevice,
    pub test: Pipeline,
    pub bench: Pipeline,
}

impl Harness {
    pub(super) fn new(name: &str) -> Result<Self> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut gpu = GpuDevice::new_raster(&[], root.join("../../target/ara-render").join(name))?;
        let artifacts = ara_gpu::cooked::resource_dir(
            env!("CARGO_PKG_NAME"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
        );
        let mut load = |name: &str| -> Result<Pipeline> {
            let cooked: ara_gpu::cooked::CookedPipeline =
                serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
            Ok(gpu.create_pipeline(&cooked, &artifacts)?)
        };
        let (test, bench) = (load("trace_test")?, load("trace_bench")?);
        Ok(Self { gpu, test, bench })
    }

    /// Device-local rays (origin, tMin, direction, tMax per ray) and their heap index.
    pub(super) fn rays(&mut self, rays: &[[f32; 8]]) -> Result<(Buffer, u32)> {
        let buffer = super::super::gpu::upload_static(&mut self.gpu, bytemuck::cast_slice(rays))?;
        Ok((buffer, self.gpu.storage_buffer(buffer)?.index))
    }

    /// Device-local result storage of `bytes` and a host readback buffer.
    pub(super) fn results(&mut self, bytes: u64) -> Result<(Buffer, u32, Buffer)> {
        let out = self.gpu.create_buffer(
            bytes,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
            false,
        )?;
        let index = self.gpu.storage_buffer(out)?.index;
        let readback = self
            .gpu
            .create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        Ok((out, index, readback))
    }

    /// One dispatch of `pipeline` over `count` rays; with `timed`, its pass time (ms).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn dispatch(
        &mut self,
        pipeline: Pipeline,
        record: u32,
        rays: u32,
        results: (Buffer, u32),
        count: u32,
        flags: u32,
        mask: u32,
        first: bool,
    ) -> Result<f64> {
        let groups = count.div_ceil(32);
        let x = groups.min(65_535);
        let y = groups.div_ceil(x);
        let mut c = self.gpu.begin(Queue::Graphics);
        c.barriers([Barrier {
            resource: Resource::Buffer(results.0),
            before: if first {
                Access::Undefined
            } else {
                Access::Storage
            },
            after: Access::Storage,
            handoff: None,
        }]);
        c.push(Command::BeginPass("trace".into()));
        c.push(Command::Pipeline(pipeline));
        c.push(Command::Constants(RootConstants {
            words: [
                record,
                rays,
                results.1,
                count,
                x * 32,
                flags,
                mask,
                0f32.to_bits(),
            ],
        }));
        c.push(Command::Dispatch([x, y, 1]));
        c.push(Command::EndPass);
        let point = self.gpu.submit(c.finish(), &[])?;
        let timings = self.gpu.resolve(point)?;
        Ok(timings
            .iter()
            .filter(|t| t.pass == "trace")
            .map(|t| t.milliseconds)
            .sum())
    }

    /// Copies `results` to `readback` and reads it.
    pub(super) fn read(
        &mut self,
        results: Buffer,
        readback: Buffer,
        bytes: u64,
    ) -> Result<Vec<u8>> {
        let mut c = self.gpu.begin(Queue::Graphics);
        c.barriers([Barrier {
            resource: Resource::Buffer(results),
            before: Access::Storage,
            after: Access::CopySource,
            handoff: None,
        }]);
        c.push(Command::CopyBuffer {
            source: results,
            destination: readback,
            size: bytes,
        });
        c.barriers([
            Barrier {
                resource: Resource::Buffer(readback),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(results),
                before: Access::CopySource,
                after: Access::Storage,
                handoff: None,
            },
        ]);
        let point = self.gpu.submit(c.finish(), &[])?;
        self.gpu.resolve(point)?;
        Ok(self.gpu.read(readback)?)
    }
}

/// One instance of the test world.
#[derive(Clone)]
struct Placed {
    mesh: TestMesh,
    world: Affine3A,
    mask: u8,
    /// Alpha material per submesh (only the card's submesh 0).
    alpha: Vec<bool>,
}

fn alpha_texels() -> Vec<u8> {
    let mut t = vec![255u8; 16 * 16 * 4];
    for y in 0..16 {
        for x in 0..16 {
            t[(y * 16 + x) * 4 + 3] = if (x / CELL + y / CELL).is_multiple_of(2) {
                255
            } else {
                0
            };
        }
    }
    t
}

/// Bilinear alpha at LOD 0 with repeat addressing (the material sampler).
fn alpha_at(texels: &[u8], uv: [f64; 2]) -> f64 {
    let (u, v) = (uv[0] * 16.0 - 0.5, uv[1] * 16.0 - 0.5);
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = (u - x0, v - y0);
    let a = |x: f64, y: f64| {
        let (x, y) = (
            (x as i64).rem_euclid(16) as usize,
            (y as i64).rem_euclid(16) as usize,
        );
        f64::from(texels[(y * 16 + x) * 4 + 3]) / 255.0
    };
    let top = a(x0, y0) * (1.0 - fx) + a(x0 + 1.0, y0) * fx;
    let bottom = a(x0, y0 + 1.0) * (1.0 - fx) + a(x0 + 1.0, y0 + 1.0) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// The reference closest hit, and whether the ray is ambiguous (an alpha candidate
/// near the cutoff, or a hit within an ulp-scale distance of a triangle edge).
struct Reference {
    hit: Option<(f64, u32, u32, bool, [f64; 2], DVec3)>,
    ambiguous: bool,
    /// An alpha test rejected a candidate in front of the result.
    rejected: bool,
}

fn reference(world: &[Placed], texels: &[u8], ray: &[f32; 8], flags: u32, mask: u32) -> Reference {
    let o = DVec3::new(ray[0].into(), ray[1].into(), ray[2].into());
    let d = DVec3::new(ray[4].into(), ray[5].into(), ray[6].into());
    let (t_min, mut t_max) = (f64::from(ray[3]), f64::from(ray[7]));
    let mut best = None;
    let mut doubtful = Vec::new();
    let mut rejections = Vec::new();
    for (i, p) in world.iter().enumerate() {
        if u32::from(p.mask) & mask == 0 {
            continue;
        }
        let m = DAffine3::from_mat4(Mat4::from(p.world).as_dmat4());
        let mesh = p.mesh.mesh();
        for t in 0..mesh.triangle_count() as u32 {
            let c = mesh.corners(t).map(|v| m.transform_point3(v.as_dvec3()));
            let Some((dist, u, v, front)) = plane_hit(o, d, &c) else {
                continue;
            };
            if dist <= t_min || dist >= t_max {
                continue;
            }
            if !front && flags & CULL_BACK != 0 {
                continue;
            }
            // Within 1e-4 of an edge, inside or out, the GPU's float test (with its
            // edge slack) may decide either way.
            let inside = u.min(v).min(1.0 - u - v);
            if inside < -1e-4 {
                continue;
            }
            let edge = inside < 1e-4;
            if inside < 0.0 {
                doubtful.push(dist);
                continue;
            }
            let submesh = mesh.submesh_of(t) as usize;
            if p.alpha[submesh] && flags & FORCE_OPAQUE == 0 {
                let ix = &p.mesh.indices[t as usize * 3..t as usize * 3 + 3];
                let uv = ix.iter().map(|&k| p.mesh.uvs[k as usize]);
                let w = [1.0 - u - v, u, v];
                let mut at = [0.0; 2];
                for (k, c) in uv.enumerate() {
                    at[0] += f64::from(c[0]) * w[k];
                    at[1] += f64::from(c[1]) * w[k];
                }
                let alpha = alpha_at(texels, at);
                if (alpha - 0.5).abs() < 0.2 {
                    doubtful.push(dist);
                }
                if alpha < 0.5 {
                    rejections.push(dist);
                    continue;
                }
            }
            if edge {
                doubtful.push(dist);
            }
            let n = (c[1] - c[0]).cross(c[2] - c[0]).normalize();
            t_max = dist;
            best = Some((dist, i as u32, t, front, [u, v], n));
        }
    }
    let end = best.map_or(f64::INFINITY, |b| b.0);
    Reference {
        hit: best,
        ambiguous: doubtful.iter().any(|&t| t <= end * (1.0 + 1e-6)),
        rejected: rejections.iter().any(|&t| t < end),
    }
}

/// f64 ray-plane hit of a triangle: distance, weights of corners 1 and 2 (possibly
/// outside the triangle) and whether the front faces the ray.
fn plane_hit(o: DVec3, d: DVec3, c: &[DVec3; 3]) -> Option<(f64, f64, f64, bool)> {
    let (e1, e2) = (c[1] - c[0], c[2] - c[0]);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - c[0];
    let q = s.cross(e1);
    Some((e2.dot(q) * inv, s.dot(p) * inv, d.dot(q) * inv, det > 0.0))
}

fn oct_decode(q: u32) -> DVec3 {
    let p = [f64::from(q & 0xffff), f64::from(q >> 16)].map(|v| v / 65535.0 * 2.0 - 1.0);
    let mut n = DVec3::new(p[0], p[1], 1.0 - p[0].abs() - p[1].abs());
    let t = (-n.z).max(0.0);
    n.x += if n.x >= 0.0 { -t } else { t };
    n.y += if n.y >= 0.0 { -t } else { t };
    n.normalize()
}

/// The world: a floor, transformed cubes (one mirrored, one hidden behind mask 2),
/// spheres and alpha-tested cards.
fn world() -> Vec<Placed> {
    let mut rng = Rng::new(17);
    let mut out = vec![Placed {
        mesh: floor(),
        world: Affine3A::IDENTITY,
        mask: 1,
        alpha: vec![false],
    }];
    for i in 0..12 {
        let axis = rng.dir();
        let mut scale = Vec3::new(rng.range(0.5, 2.), rng.range(0.5, 3.), rng.range(0.5, 2.));
        if i == 3 {
            scale.x = -scale.x;
        }
        let t = Vec3::new(
            rng.range(-15., 15.),
            rng.range(0.5, 3.),
            rng.range(-15., 15.),
        );
        out.push(Placed {
            mesh: cube(),
            world: Affine3A::from_scale_rotation_translation(
                scale,
                Quat::from_axis_angle(axis, rng.range(0., 6.)),
                t,
            ),
            mask: if i == 7 { 2 } else { 1 },
            alpha: vec![false],
        });
    }
    for _ in 0..4 {
        let t = Vec3::new(
            rng.range(-15., 15.),
            rng.range(1., 4.),
            rng.range(-15., 15.),
        );
        out.push(Placed {
            mesh: sphere(),
            world: Affine3A::from_scale_rotation_translation(
                Vec3::splat(rng.range(0.5, 2.)),
                Quat::IDENTITY,
                t,
            ),
            mask: 1,
            alpha: vec![false],
        });
    }
    for _ in 0..6 {
        let t = Vec3::new(rng.range(-12., 12.), 0., rng.range(-12., 12.));
        out.push(Placed {
            mesh: card(),
            world: Affine3A::from_scale_rotation_translation(
                Vec3::splat(rng.range(1., 2.5)),
                Quat::from_rotation_y(rng.range(0., 6.)),
                t,
            ),
            mask: 1,
            alpha: vec![true, false],
        });
    }
    out
}

/// Random rays plus rays aimed at random triangles, some with clipped t ranges.
fn rays(world: &[Placed], n: usize, seed: u64) -> Vec<[f32; 8]> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|i| {
            let o = Vec3::new(
                rng.range(-25., 25.),
                rng.range(0.3, 10.),
                rng.range(-25., 25.),
            );
            let d = if i % 2 == 0 {
                rng.dir()
            } else {
                let p = &world[rng.next() as usize % world.len()];
                let mesh = p.mesh.mesh();
                let c = mesh.corners(rng.next() as u32 % mesh.triangle_count() as u32);
                let (a, b) = (rng.range(0., 1.), rng.range(0., 1.));
                let (a, b) = if a + b > 1. { (1. - a, 1. - b) } else { (a, b) };
                let target = p
                    .world
                    .transform_point3(c[0] + (c[1] - c[0]) * a + (c[2] - c[0]) * b);
                (target - o).normalize_or(Vec3::Y) * if i % 6 == 1 { 2.5 } else { 1.0 }
            };
            let (t_min, t_max) = match i % 7 {
                3 => (1.5, 1e30),
                5 => (0.0, 12.0),
                _ => (0.0, 1e30),
            };
            [o.x, o.y, o.z, t_min, d.x, d.y, d.z, t_max]
        })
        .collect()
}

/// Builds the trace scene of `world` with the alpha texture at heap `texture`.
fn scene_of(world: &[Placed], texture: u32) -> Result<TraceScene> {
    let mut scene = TraceScene::default();
    scene.set_alpha_material(
        0,
        Some(AlphaMaterial {
            texture,
            uv_set: 0,
            cutoff: 0.5,
            factor: 1.0,
            offset: [0.0; 2],
            scale: [1.0; 2],
            rotation: 0.0,
        }),
    );
    for (i, p) in world.iter().enumerate() {
        let alpha = p.alpha.iter().any(|&a| a);
        let mesh = p.mesh.trace(alpha);
        scene.add_mesh(mesh.clone());
        scene.set_instance(
            1000 + i as u64,
            TraceInstance {
                mesh: mesh.blas.hash,
                world: p.world,
                mask: p.mask,
                materials: p
                    .alpha
                    .iter()
                    .map(|&a| if a { 0 } else { NO_MATERIAL })
                    .collect(),
                user: 500 + i as u32,
            },
        )?;
    }
    scene.commit();
    Ok(scene)
}

/// Runs `rays` and checks every unambiguous ray against the reference.
#[allow(clippy::too_many_arguments)]
fn check(
    h: &mut Harness,
    trace: &TraceGpu,
    world: &[Placed],
    texels: &[u8],
    rays: &[[f32; 8]],
    flags: u32,
    mask: u32,
) -> Result<(usize, usize)> {
    let (ray_buffer, ray_index) = h.rays(rays)?;
    let bytes = rays.len() as u64 * 32;
    let (out, out_index, readback) = h.results(bytes)?;
    h.dispatch(
        h.test,
        trace.record(),
        ray_index,
        (out, out_index),
        rays.len() as u32,
        flags,
        mask,
        true,
    )?;
    let words: Vec<u32> = bytemuck::cast_slice(&h.read(out, readback, bytes)?).to_vec();
    let (mut hits, mut alpha_rejections) = (0, 0);
    for (i, ray) in rays.iter().enumerate() {
        let w = &words[i * 8..i * 8 + 8];
        let want = reference(world, texels, ray, flags, mask);
        if want.ambiguous {
            continue;
        }
        let t = f32::from_bits(w[0]);
        let found = t >= 0.0;
        ensure!(
            found == want.hit.is_some(),
            "ray {i} (flags {flags}): GPU found {found}, reference {:?}",
            want.hit.map(|h| (h.0, h.1, h.2))
        );
        if flags & ACCEPT_FIRST != 0 {
            continue;
        }
        let Some((rt, instance, primitive, front, bary, normal)) = want.hit else {
            continue;
        };
        hits += 1;
        let tol = 2e-4 + 2e-5 * rt;
        ensure!(
            (f64::from(t) - rt).abs() <= tol,
            "ray {i} flags {flags}: t {t} (instance {}, primitive {}) vs {rt} (instance {instance}, primitive {primitive}, front {front}, mirrored {}, alpha {:?}) ray {ray:?}",
            w[1],
            w[2],
            world[instance as usize].world.matrix3.determinant() < 0.0,
            world[instance as usize].alpha
        );
        if (w[1], w[2]) == (instance, primitive) {
            ensure!(w[3] >> 31 == u32::from(front), "ray {i}: orientation");
            ensure!(
                w[3] & 0x7fff_ffff == world[instance as usize].mesh.mesh().submesh_of(primitive),
                "ray {i}: submesh"
            );
            ensure!(w[6] == 500 + instance, "ray {i}: caller word");
            let b = [
                f64::from(f32::from_bits(w[4])),
                f64::from(f32::from_bits(w[5])),
            ];
            ensure!(
                (b[0] - bary[0]).abs() < 1e-3 && (b[1] - bary[1]).abs() < 1e-3,
                "ray {i}: barycentrics {b:?} vs {bary:?}"
            );
            let n = oct_decode(w[7]);
            ensure!(n.dot(normal) > 0.999, "ray {i}: normal {n} vs {normal}");
        }
        if want.rejected {
            alpha_rejections += 1;
        }
    }
    for b in [ray_buffer, out, readback] {
        h.gpu.retire(Resource::Buffer(b))?;
    }
    Ok((hits, alpha_rejections))
}

#[test]
fn gpu_trace_matches_world_brute_force() -> Result<()> {
    let mut h = Harness::new("trace-gpu")?;
    let texels = alpha_texels();
    let image =
        crate::renderer::upload_image(&mut h.gpu, [16, 16], vk::Format::R8G8B8A8_UNORM, &texels)?;
    let texture = h.gpu.sampled_image(image, false)?.index;
    let mut world = world();
    let mut scene = scene_of(&world, texture)?;
    let mut trace = TraceGpu::new(&mut h.gpu, &scene)?;
    assert_eq!(trace.indices().flags, SCENE_NONEMPTY | SCENE_ALPHA);
    // Generation words of every instance record.
    let records: Vec<u32> = bytemuck::cast_slice(&h.gpu.read(trace.buffers()[4])?).to_vec();
    for (slot, s) in scene.instance_slots().iter().enumerate() {
        let s = s.as_ref().expect("dense slots");
        let w = &records[slot * 20..slot * 20 + 20];
        assert_eq!(
            u64::from(w[18]) | (u64::from(w[19]) << 32),
            s.key.generation
        );
        assert_eq!(w[16], s.instance.user);
    }
    let rays = rays(&world, 4096, 5);
    let (hits, alpha) = check(&mut h, &trace, &world, &texels, &rays, 0, 0xff)?;
    assert!(
        hits > 1500 && alpha > 20,
        "fixture coverage: {hits} hits, {alpha} alpha rejections"
    );
    check(&mut h, &trace, &world, &texels, &rays, CULL_BACK, 1)?;
    check(&mut h, &trace, &world, &texels, &rays, ACCEPT_FIRST, 0xff)?;
    check(&mut h, &trace, &world, &texels, &rays, FORCE_OPAQUE, 0xff)?;
    // Moves refit in place: no allocation, same record.
    let usage = h.gpu.descriptor_usage();
    let record = trace.record();
    for (i, p) in world.iter_mut().enumerate().skip(1).step_by(2) {
        p.world.translation += glam::Vec3A::new(2.5, 0.5, -1.0);
        assert!(scene.set_transform(1000 + i as u64, p.world));
    }
    assert_eq!(scene.commit(), Commit::Refit);
    assert_eq!(trace.update(&mut h.gpu, &scene)?, Upload::Rewritten);
    assert_eq!(
        (h.gpu.descriptor_usage(), trace.record()),
        (usage, record),
        "refits allocate nothing"
    );
    check(&mut h, &trace, &world, &texels, &rays, 0, 0xff)?;
    // A new instance republishes.
    let extra = Placed {
        mesh: sphere(),
        world: Affine3A::from_scale_rotation_translation(
            Vec3::splat(3.),
            Quat::IDENTITY,
            Vec3::new(0., 3., 0.),
        ),
        mask: 1,
        alpha: vec![false],
    };
    let mesh = extra.mesh.trace(false);
    scene.add_mesh(mesh.clone());
    scene.set_instance(
        1000 + world.len() as u64,
        TraceInstance {
            mesh: mesh.blas.hash,
            world: extra.world,
            mask: 1,
            materials: vec![NO_MATERIAL],
            user: 500 + world.len() as u32,
        },
    )?;
    world.push(extra);
    scene.commit();
    assert_eq!(trace.update(&mut h.gpu, &scene)?, Upload::Republished);
    check(&mut h, &trace, &world, &texels, &rays, 0, 0xff)?;
    trace.destroy(&mut h.gpu)?;
    h.gpu.retire(Resource::Image(image))?;
    h.gpu.retire_pipeline(h.test)?;
    h.gpu.retire_pipeline(h.bench)?;
    h.gpu.collect()?;
    h.gpu.assert_clean()?;
    Ok(())
}
