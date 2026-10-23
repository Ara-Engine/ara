//! Bounded OBJ/MTL benchmark import, deliberately separate from engine asset cooking.
use super::*;
use anyhow::{Context, ensure};
use ara_gpu::cooked::write_json;
use ara_render::scene::{CameraSpec, Material, Mesh, Scene, Texture, Vertex};
use glam::Vec3;
use std::collections::BTreeMap;

/// Load the pinned OBJ/MTL benchmark into the shared scene contract.
pub(crate) fn load(path: &Path) -> Result<Scene> {
    let mut scene = scenes::scene("glossy_interior", 0);
    scene.meshes.clear();
    scene.materials.clear();
    scene.lights.clear();
    scene.sun_direction = Vec3::new(-0.3, 0.8, 0.45).normalize();
    scene.sun_lux = Vec3::new(18000., 16000., 13000.);
    scene.environment = vec![[100., 140., 220., 1.]; 8];
    let mut names = BTreeMap::new();
    let mut textures = BTreeMap::new();
    for line in std::fs::read_to_string(path.join("sponza.mtl"))?.lines() {
        let words = line.split_whitespace().collect::<Vec<_>>();
        let Some(key) = words.first() else { continue };
        if *key == "newmtl" {
            ensure!(words.len() == 2, "Invalid material name");
            names.insert(words[1].to_owned(), scene.materials.len());
            scene.materials.push(Material {
                base_color: [1.; 4],
                emissive_roughness: [0., 0., 0., 0.7],
                ..Material::default()
            });
            continue;
        }
        let Some(material) = scene.materials.last_mut() else {
            continue;
        };
        match *key {
            "Kd" if words.len() == 4 => {
                for (c, word) in material.base_color.iter_mut().zip(&words[1..]) {
                    *c = word.parse()?;
                }
            }
            "Ns" if words.len() == 2 => {
                material.emissive_roughness[3] = (2. / (words[1].parse::<f32>()? + 2.))
                    .powf(0.25)
                    .clamp(0.15, 1.);
            }
            "map_d" => material.flags[0] = 1,
            "map_Kd" if words.len() == 2 => {
                let name = words[1].replace('\\', "/");
                let index = if let Some(index) = textures.get(&name) {
                    *index
                } else {
                    let image = image::open(path.join(&name))?.to_rgba8();
                    let index = scene.textures.len() as u32;
                    scene.textures.push(Texture {
                        size: [image.width(), image.height()],
                        rgba: image.into_raw(),
                        srgb: true,
                    });
                    textures.insert(name, index);
                    index
                };
                material.textures[0] = index;
            }
            _ => {}
        }
    }
    let mut positions = Vec::<Vec3>::new();
    let mut normals = Vec::<Vec3>::new();
    let mut uvs = Vec::<[f32; 2]>::new();
    let mut meshes = (0..scene.materials.len())
        .map(|material| Mesh {
            entity: material as u64 + 1,
            name: format!("Sponza material {material}"),
            vertices: Vec::new(),
            indices: Vec::new(),
            material,
        })
        .collect::<Vec<_>>();
    let mut indices = vec![BTreeMap::new(); meshes.len()];
    let mut material = 0;
    for line in std::fs::read_to_string(path.join("sponza.obj"))?.lines() {
        let words = line.split_whitespace().collect::<Vec<_>>();
        let Some(key) = words.first() else { continue };
        match *key {
            "v" | "vn" if words.len() >= 4 => {
                let value = Vec3::new(words[1].parse()?, words[2].parse()?, words[3].parse()?);
                if *key == "v" {
                    positions.push(value * 0.01);
                } else {
                    normals.push(value.normalize());
                }
            }
            "vt" if words.len() >= 3 => {
                uvs.push([words[1].parse()?, 1. - words[2].parse::<f32>()?])
            }
            "usemtl" if words.len() == 2 => {
                material = *names.get(words[1]).context("Unknown OBJ material")?
            }
            "f" if words.len() >= 4 => {
                let mut face = Vec::new();
                for word in &words[1..] {
                    let key = word
                        .split('/')
                        .map(str::parse::<usize>)
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    ensure!(
                        key.len() == 3 && key.iter().all(|i| *i > 0),
                        "Expected positive position/UV/normal OBJ indices"
                    );
                    let index = if let Some(index) = indices[material].get(&key) {
                        *index
                    } else {
                        let position = *positions.get(key[0] - 1).context("OBJ position index")?;
                        let uv = *uvs.get(key[1] - 1).context("OBJ UV index")?;
                        let normal = *normals.get(key[2] - 1).context("OBJ normal index")?;
                        let tangent = normal
                            .cross(if normal.y.abs() < 0.99 {
                                Vec3::Y
                            } else {
                                Vec3::X
                            })
                            .normalize();
                        let index = meshes[material].vertices.len() as u32;
                        meshes[material].vertices.push(Vertex {
                            position: position.to_array(),
                            normal: normal.to_array(),
                            uv,
                            tangent: tangent.extend(1.).to_array(),
                        });
                        indices[material].insert(key, index);
                        index
                    };
                    face.push(index);
                }
                for i in 1..face.len() - 1 {
                    meshes[material]
                        .indices
                        .extend([face[0], face[i], face[i + 1]]);
                }
            }
            _ => {}
        }
    }
    scene.meshes = meshes
        .into_iter()
        .filter(|m| !m.indices.is_empty())
        .collect();
    Ok(scene)
}

/// Pinned same-view comparison camera, in metres.
pub(crate) fn camera() -> CameraSpec {
    CameraSpec::LookAt {
        position: Vec3::new(-11., 1.7, 0.),
        target: Vec3::new(5., 3., 0.),
        vertical_fov: 1.05,
    }
}

/// Measure locally fetched Sponza content with the complete RT-tier frame.
pub fn run(gpu: &mut VulkanDevice, root: &Path) -> Result<()> {
    let path = root.join(".sn/assets/sponza");
    let scene =
        load(&path).context("Fetch the local Sponza benchmark with fetch-sponza.ps1 first")?;
    let camera = camera();
    let mut report = Vec::new();
    let mut budgets_passed = true;
    for render in [[2560, 1440], [1707, 960]] {
        let mut s = settings(render, [2560, 1440]);
        s.post = PostSettings {
            ev100: None,
            compensation: -1.5,
            tone_mapper: ara_render_fidelity::ToneMapper::PbrNeutral,
            ..PostSettings::default()
        };
        let mut view = FidelityView::new(gpu, &scene, s, None)?;
        view.set_gather_events(1)?;
        view.set_transport_stride(4)?;
        let mut passes = BTreeMap::<String, Vec<f64>>::new();
        let mut totals = Vec::new();
        let background_before = background_load();
        for tick in 0..336 {
            let mut request = input(&camera, tick, false);
            request.deterministic = false;
            view.render(gpu, &request)?;
            if tick >= 96 {
                let mut total = 0.;
                for timing in view.timings() {
                    passes
                        .entry(timing.pass.to_owned())
                        .or_default()
                        .push(timing.milliseconds);
                    total += timing.milliseconds;
                }
                totals.push(total);
            }
        }
        view.capture(gpu)?
            .save(root.join(format!("captures/sponza-{}p.png", render[1])))?;
        totals.sort_by(f64::total_cmp);
        let p50 = totals[120];
        let p95 = totals[227];
        let budget = if render[1] == 1440 { 10. } else { 5. };
        budgets_passed &= p95 <= budget;
        println!("Sponza {}p: p50 {p50:.3} ms, p95 {p95:.3} ms", render[1]);
        let passes = passes
            .into_iter()
            .map(|(name, mut times)| {
                times.sort_by(f64::total_cmp);
                (name, serde_json::json!({"p50":times[120],"p95":times[227]}))
            })
            .collect::<BTreeMap<_, _>>();
        report.push(serde_json::json!({"source":"https://casual-effects.com/g3d/data10/common/model/crytek_sponza/sponza.zip","attribution":"Frank Meinl, Crytek 2010; Morgan McGuire Computer Graphics Archive, 2011 corrections","archive_sha256":"da005cbee0be2df2abc8513f3ceb61bcb6f69aac112babcd9c00169a27c2770c","asset_policy":"local benchmark; original copyright.txt retained; no redistributed model files","triangle_count":scene.meshes.iter().map(|m|m.indices.len()/3).sum::<usize>(),"instances":scene.meshes.len(),"materials":scene.materials.len(),"textures":scene.textures.len(),"material_conversion":"OBJ Kd and PNG alpha; Ns -> GGX roughness; authored vertex normals; height maps not imported","render_size":render,"display_size":[2560,1440],"frames":240,"warmup":96,"p50":p50,"p95":p95,"budget_ms":budget,"budget_passed":p95<=budget,"passes":passes,"camera":camera,"scene_hash":view.scene_hash(),"async_overlap":false,"exposure":view.capture_exposure(gpu)?,"gather_events":1,"transport_stride":4,"background_before":background_before,"background_after":background_load(),"background_load":"shared RTX 4070 SUPER with approximately ten other agents; no exclusive GPU reservation or forced clocks"}));
        view.destroy(gpu)?;
    }
    write_json(&root.join("measurements/sponza-timings.json"), &report)?;
    gpu.assert_clean()?;
    ensure!(
        budgets_passed,
        "Sponza p95 budget failed; see sponza-timings.json"
    );
    Ok(())
}

/// Benchmark-only telemetry; no machine query enters the renderer's frame loop.
pub(crate) fn background_load() -> String {
    std::process::Command::new("nvidia-smi").args(["--query-gpu=utilization.gpu,utilization.memory,memory.used,power.draw,clocks.gr,clocks.mem","--format=csv,noheader"]).output().map(|o|String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_else(|e|format!("unavailable: {e}"))
}
