//! `trace()` timing on real Ara content: the Showcase sample's geometry (stage,
//! plinths, 18 material balls, 11 hero props and the mannequin, each glTF primitive
//! its own BLAS under the scene's transforms). Two native 2560x1440 sets from the
//! scene camera's position looking at the content's centre: primary rays (8x4 pixel
//! tiles, as R4-1's camera sets) and full-pixel mirror reflections off those hits.
//! Both are checked against a CPU two-level reference on 4,096 rays before timing.
//! An ignored test: run under the quiet-window lock, like `bench`.
//! `ARA_SHOWCASE_ASSETS` points at the fetched sample assets (`kit/`, `third_party/`).
use super::gpu::Harness;
use crate::look::trace::{NO_MATERIAL, TraceGpu, TraceInstance, TraceMesh, TraceScene};
use anyhow::{Context, Result, ensure};
use ara_bvh::{Blas, Mesh, Ray, RayFlags};
use ara_gpu::{Device, Resource};
use glam::{Affine3A, EulerRot, Mat4, Quat, Vec3};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// One placed primitive: its mesh and object-to-world transform.
struct Placed {
    mesh: TraceMesh,
    world: Affine3A,
}

/// A glTF primitive: positions, indices and its node-to-model transform.
type Primitive = (Vec<[f32; 3]>, Vec<u32>, Mat4);

/// Every triangle primitive of a glTF file, flattened through its default scene's
/// node transforms.
fn load_gltf(path: &Path) -> Result<Vec<Primitive>> {
    let gltf = gltf::Gltf::open(path).with_context(|| path.display().to_string())?;
    let buffers = gltf::import_buffers(&gltf.document, path.parent(), gltf.blob.clone())?;
    let mut out = Vec::new();
    let scene = gltf
        .document
        .default_scene()
        .or_else(|| gltf.document.scenes().next())
        .context("glTF scene")?;
    let mut stack: Vec<(gltf::Node, Mat4)> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            for p in mesh
                .primitives()
                .filter(|p| p.mode() == gltf::mesh::Mode::Triangles)
            {
                let reader = p.reader(|b| Some(&buffers[b.index()]));
                let Some(positions) = reader.read_positions() else {
                    continue;
                };
                let positions: Vec<[f32; 3]> = positions.collect();
                let indices: Vec<u32> = match reader.read_indices() {
                    Some(i) => i.into_u32().collect(),
                    None => (0..positions.len() as u32).collect(),
                };
                out.push((positions, indices, world));
            }
        }
        stack.extend(node.children().map(|c| (c, world)));
    }
    Ok(out)
}

/// The Showcase scene's mesh renderers as placed primitives.
fn showcase(assets: &Path) -> Result<(Vec<Placed>, Vec3)> {
    let scene_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples/showcase/content/levels/showcase.scene");
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&scene_path)?)?;
    let records = doc["records"].as_object().context("scene records")?;
    let mut cache: BTreeMap<String, Vec<(TraceMesh, Mat4)>> = BTreeMap::new();
    let mut placed = Vec::new();
    let mut camera = Vec3::new(0.0, 2.2, -7.5);
    let vec3 = |v: &serde_json::Value, default: f32| -> Vec3 {
        let a = v.as_array();
        let f = |i: usize| {
            a.and_then(|a| a.get(i))
                .and_then(serde_json::Value::as_f64)
                .map_or(default, |x| x as f32)
        };
        Vec3::new(f(0), f(1), f(2))
    };
    for record in records.values() {
        let t = &record["Transform"];
        let world = Mat4::from_scale_rotation_translation(
            vec3(&t["scale"], 1.0),
            {
                let r = vec3(&t["rotation"], 0.0) * (std::f32::consts::PI / 180.0);
                Quat::from_euler(EulerRot::XYZ, r.x, r.y, r.z)
            },
            vec3(&t["position"], 0.0),
        );
        if record.get("Camera").is_some() {
            camera = vec3(&t["position"], 0.0);
        }
        let Some(model) = record["MeshRenderer"]["model"].as_str() else {
            continue;
        };
        let file = model
            .trim_start_matches('@')
            .split('#')
            .next()
            .unwrap_or_default()
            .to_string();
        if !cache.contains_key(&file) {
            let mut meshes = Vec::new();
            for (positions, indices, node) in load_gltf(&assets.join(&file))? {
                let blas = Blas::build(&Mesh {
                    positions: &positions,
                    indices: &indices,
                    submeshes: &[],
                })?;
                meshes.push((
                    TraceMesh {
                        blas: Arc::new(blas),
                        uvs: None,
                    },
                    node,
                ));
            }
            cache.insert(file.clone(), meshes);
        }
        for (mesh, node) in &cache[&file] {
            placed.push(Placed {
                mesh: mesh.clone(),
                world: Affine3A::from_mat4(world * *node),
            });
        }
    }
    Ok((placed, camera))
}

/// Rays through pixel centres of a `w` x `h` camera at `eye` looking at `target`
/// (45 degree vertical field), ordered in 8x4 tiles.
fn camera_rays(eye: Vec3, target: Vec3, w: u32, h: u32) -> Vec<[f32; 8]> {
    let forward = (target - eye).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    let tan = (22.5f32).to_radians().tan();
    let aspect = w as f32 / h as f32;
    let mut out = Vec::with_capacity((w * h) as usize);
    for ty in 0..h / 4 {
        for tx in 0..w / 8 {
            for y in ty * 4..ty * 4 + 4 {
                for x in tx * 8..tx * 8 + 8 {
                    let u = ((x as f32 + 0.5) / w as f32 * 2.0 - 1.0) * tan * aspect;
                    let v = (1.0 - (y as f32 + 0.5) / h as f32 * 2.0) * tan;
                    let d = (forward + right * u + up * v).normalize();
                    out.push([eye.x, eye.y, eye.z, 0.0, d.x, d.y, d.z, 1e30]);
                }
            }
        }
    }
    out
}

/// CPU closest hit over every placed primitive (BLAS traversal in object space).
fn cpu_hit(placed: &[Placed], ray: &[f32; 8]) -> Option<f32> {
    let (o, d) = (
        Vec3::new(ray[0], ray[1], ray[2]),
        Vec3::new(ray[4], ray[5], ray[6]),
    );
    let mut best: Option<f32> = None;
    for p in placed {
        let inv = p.world.inverse();
        let r = Ray {
            origin: inv.transform_point3(o),
            direction: inv.transform_vector3(d),
            t_min: ray[3],
            t_max: best.unwrap_or(ray[7]),
        };
        if let Some(h) = p.mesh.blas.intersect(&r, RayFlags::NONE) {
            best = Some(h.t);
        }
    }
    best
}

fn oct_decode(q: u32) -> Vec3 {
    let p = [(q & 0xffff) as f32, (q >> 16) as f32].map(|v| v / 65535.0 * 2.0 - 1.0);
    let mut n = Vec3::new(p[0], p[1], 1.0 - p[0].abs() - p[1].abs());
    let t = (-n.z).max(0.0);
    n.x += if n.x >= 0.0 { -t } else { t };
    n.y += if n.y >= 0.0 { -t } else { t };
    n.normalize()
}

#[test]
#[ignore = "trace() timing on the Showcase sample's geometry; run under the quiet-window lock"]
fn trace_timing_showcase() -> Result<()> {
    let assets = std::env::var("ARA_SHOWCASE_ASSETS")
        .unwrap_or_else(|_| "D:/AraBuild/stale-sample-assets/showcase-assets".into());
    let reps: usize = std::env::var("ARA_TRACE_REPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    let start = std::time::Instant::now();
    let (placed, eye) = showcase(Path::new(&assets))?;
    let build_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut scene = TraceScene::default();
    let mut bounds = ara_bvh::Aabb::EMPTY;
    let mut triangles = 0u64;
    for (i, p) in placed.iter().enumerate() {
        scene.add_mesh(p.mesh.clone());
        scene.set_instance(
            i as u64,
            TraceInstance {
                mesh: p.mesh.blas.hash,
                world: p.world,
                mask: 1,
                materials: vec![NO_MATERIAL],
                user: i as u32,
            },
        )?;
        if p.mesh.blas.bounds.is_valid() {
            bounds = bounds.union(p.mesh.blas.bounds.transformed(&p.world));
        }
        triangles += u64::from(p.mesh.blas.stats.triangles);
    }
    scene.commit();
    let mut h = Harness::new("trace-showcase")?;
    let trace = TraceGpu::new(&mut h.gpu, &scene)?;
    let blas_bytes = trace.blas_bytes(&scene);
    eprintln!(
        "showcase: {} instances, {triangles} instanced triangles, BLAS pool {blas_bytes} B, CPU builds {build_ms:.0} ms",
        placed.len()
    );
    let (w, hgt) = (2560, 1440);
    let primary = camera_rays(eye, bounds.center(), w, hgt);
    // Primary hits (with normals) give the reflection set.
    let n = primary.len() as u32;
    let (rays, ray_index) = h.rays(&primary)?;
    let (out, out_index, readback) = h.results(u64::from(n) * 32)?;
    h.dispatch(
        h.test,
        trace.record(),
        ray_index,
        (out, out_index),
        n,
        0,
        0xff,
        true,
    )?;
    let words: Vec<u32> = bytemuck::cast_slice(&h.read(out, readback, u64::from(n) * 32)?).to_vec();
    for b in [rays, out, readback] {
        h.gpu.retire(Resource::Buffer(b))?;
    }
    let mut reflections = Vec::new();
    for (i, r) in primary.iter().enumerate() {
        let t = f32::from_bits(words[i * 8]);
        if t < 0.0 {
            continue;
        }
        let d = Vec3::new(r[4], r[5], r[6]);
        let mut nrm = oct_decode(words[i * 8 + 7]);
        if nrm.dot(d) > 0.0 {
            nrm = -nrm;
        }
        let p = Vec3::new(r[0], r[1], r[2]) + d * t + nrm * 1e-3;
        let rd = d - 2.0 * d.dot(nrm) * nrm;
        reflections.push([p.x, p.y, p.z, 0.0, rd.x, rd.y, rd.z, 1e30]);
    }
    let mut rows = Vec::new();
    for (name, set) in [
        ("showcase-primary-1440", &primary),
        ("showcase-reflect-1440", &reflections),
    ] {
        let n = set.len() as u32;
        let (rays, ray_index) = h.rays(set)?;
        let bytes = u64::from(n) * 16;
        let (out, out_index, readback) = h.results(bytes)?;
        h.dispatch(
            h.bench,
            trace.record(),
            ray_index,
            (out, out_index),
            n,
            0,
            0xff,
            true,
        )?;
        let words: Vec<u32> = bytemuck::cast_slice(&h.read(out, readback, bytes)?).to_vec();
        let (mut mismatches, mut far) = (0, 0);
        for k in 0..4096u64 {
            let i = (k * u64::from(n) / 4096) as usize;
            let got = f32::from_bits(words[i * 4]);
            let want = cpu_hit(&placed, &set[i]);
            match (got >= 0.0, want) {
                (true, Some(w)) if (got - w).abs() > 1e-4f32.max(w * 1e-5) => far += 1,
                (true, Some(_)) | (false, None) => {}
                _ => mismatches += 1,
            }
        }
        let mut times = Vec::with_capacity(reps);
        for k in 0..reps + 10 {
            let ms = h.dispatch(
                h.bench,
                trace.record(),
                ray_index,
                (out, out_index),
                n,
                0,
                0xff,
                false,
            )?;
            if k >= 10 {
                times.push(ms);
            }
        }
        times.sort_by(f64::total_cmp);
        let pct = |p: usize| times[(times.len() * p).div_ceil(100) - 1];
        eprintln!(
            "{name:22} {n:>8} rays  p50 {:.3} ms  p99 {:.3} ms  max {:.3}  refs: {mismatches} hit/miss, {far} t failures",
            pct(50),
            pct(99),
            times[times.len() - 1]
        );
        rows.push(serde_json::json!({
            "set": name, "rays": n, "p50_ms": pct(50), "p99_ms": pct(99), "max_ms": times[times.len() - 1],
            "samples": times.len(), "ref_mismatches": mismatches, "ref_t_failures": far,
        }));
        ensure!(
            mismatches == 0 && far == 0,
            "{name}: {mismatches} hit/miss, {far} distance mismatches"
        );
        for b in [rays, out, readback] {
            h.gpu.retire(Resource::Buffer(b))?;
        }
    }
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-render/trace-bench");
    std::fs::create_dir_all(&out_dir)?;
    let report = serde_json::json!({
        "device": h.gpu.info().name.clone(), "instances": placed.len(), "triangles": triangles,
        "blas_bytes": blas_bytes, "cpu_build_ms": build_ms, "rows": rows,
    });
    std::fs::write(
        out_dir.join("showcase.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    trace.destroy(&mut h.gpu)?;
    Ok(())
}
