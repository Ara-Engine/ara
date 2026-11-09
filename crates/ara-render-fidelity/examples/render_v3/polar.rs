//! Real Polar glTF in a fixed lab light rig. Uses the existing cooked texture
//! generation read-only; geometry remains local with explicit node instances.
use super::fixture::Fixture;
use anyhow::{Context, Result, ensure};
use ara_render::{
    instanced::*,
    scene::{CameraSpec, Light},
};
use ara_texture_stream::{NativeConfig, NativeResidency, NativeSource};
use glam::{Mat4, Quat, Vec3};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

fn array<const N: usize>(v: &Value, default: [f32; N]) -> [f32; N] {
    std::array::from_fn(|i| v[i].as_f64().map_or(default[i], |x| x as f32))
}
fn index(v: &Value) -> Result<usize> {
    usize::try_from(v.as_u64().context("Polar index")?).context("Polar index overflow")
}

/// Float accessor decoder for the pinned exported fixture, with bounds validation.
fn floats<const N: usize>(doc: &Value, bin: &[u8], a: &Value) -> Result<Vec<[f32; N]>> {
    let a = &doc["accessors"][index(a)?];
    ensure!(
        a["componentType"] == 5126 && a["sparse"].is_null(),
        "Polar needs dense float attributes"
    );
    let view = &doc["bufferViews"][index(&a["bufferView"])?];
    let start = index(&view["byteOffset"]).unwrap_or(0) + index(&a["byteOffset"]).unwrap_or(0);
    let stride = index(&view["byteStride"]).unwrap_or(N * 4);
    let count = index(&a["count"])?;
    ensure!(
        count > 0 && stride >= N * 4 && start + (count - 1) * stride + N * 4 <= bin.len(),
        "Polar accessor bounds"
    );
    Ok((0..count)
        .map(|i| {
            std::array::from_fn(|j| {
                let p = start + i * stride + j * 4;
                f32::from_le_bytes([bin[p], bin[p + 1], bin[p + 2], bin[p + 3]])
            })
        })
        .collect())
}

pub fn residency(f: &Fixture) -> Result<NativeResidency> {
    let mut resident = NativeResidency::new(NativeConfig {
        resident_bytes: 2 * 1024 * 1024 * 1024,
        upload_bytes: 2 * 1024 * 1024 * 1024,
    })?;
    for (key, source) in &f.textures {
        resident.register(*key, Arc::clone(source))?;
    }
    Ok(resident)
}

pub fn load() -> Result<Fixture> {
    let project = std::env::var_os("ARA_LAB_POLAR")
        .map(PathBuf::from)
        .unwrap_or_else(|| "D:/AraScratch/AraPolar".into());
    let action = std::env::var("ARA_LAB_POLAR_ACTION").unwrap_or_else(|_| {
        "f517051c345d4fde311159a1d094f12b52d79621eeff10fcfec1234a1fdab852".into()
    });
    ensure!(
        action.len() == 64 && action.bytes().all(|b| b.is_ascii_hexdigit()),
        "Polar action must be a cache digest"
    );
    let cache = project.join(".ara/cache");
    let manifest: Value =
        serde_json::from_slice(&fs::read(cache.join("actions").join(action)).context(
            "Polar cooked generation missing; set ARA_LAB_POLAR and ARA_LAB_POLAR_ACTION",
        )?)?;
    let artifacts = manifest["artifacts"]
        .as_array()
        .context("Polar artifacts")?;
    let artifact = |name: &str| -> Result<([u8; 32], Vec<u8>)> {
        let a = artifacts
            .iter()
            .find(|a| a["name"] == name)
            .context("Polar artifact missing")?;
        let key: [u8; 32] = serde_json::from_value(a["hash"].clone())?;
        let hex = key.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let bytes = fs::read(cache.join("objects").join(hex))?;
        ensure!(
            *blake3::hash(&bytes).as_bytes() == key,
            "Polar artifact digest mismatch"
        );
        Ok((key, bytes))
    };
    let (_, bytes) = artifact("scene.json")?;
    let cooked: Value = serde_json::from_slice(&bytes)?;
    let source = project.join("content/polar");
    // The importer fills glTF defaults and serializes f32 values; its document
    // is the authoritative material/node description for the cooked generation.
    let doc = &cooked["document"];
    let bin = fs::read(source.join("polar.bin"))?;
    ensure!(
        format!("{:x}", Sha256::digest(&bin))
            == "5a48db8e3d66cd9e4d72e3060893e8c59d34f912cfcf426142f79102c06dc97e",
        "Polar source buffer differs from the versioned fixture"
    );
    let mut textures = Vec::new();
    let mut texture_ids = BTreeMap::new();
    for t in cooked["textures"]
        .as_array()
        .context("Polar texture manifest")?
    {
        let (key, bytes) = artifact(t["artifact"].as_str().context("Polar texture name")?)?;
        let id = textures.len() as u32;
        textures.push((key, Arc::new(NativeSource::from_cooked(bytes.into())?)));
        texture_ids.insert(
            (
                index(&t["image"])?,
                t["usage"].as_str().context("Polar usage")?.to_owned(),
            ),
            id,
        );
    }
    let texture = |t: &Value, usage: &str| -> Result<u32> {
        let Some(i) = t["index"].as_u64() else {
            return Ok(NO_TEXTURE);
        };
        let image = index(&doc["textures"][i as usize]["source"])?;
        texture_ids
            .get(&(image, usage.to_owned()))
            .copied()
            .context("Polar cooked channel missing")
    };
    let mut scene = Scene {
        geometry: Vec::new(),
        instances: Vec::new(),
        materials: Vec::new(),
        textures: textures.len() as u32,
        decals: Vec::new(),
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ZERO,
        environment: vec![[0.1, 0.12, 0.16, 1.]],
        environment_size: [1, 1],
    };
    for m in doc["materials"].as_array().context("Polar materials")? {
        let p = &m["pbrMetallicRoughness"];
        let mut out = Material::default();
        out.pbr.base_color = array(&p["baseColorFactor"], [1.; 4]);
        let emission = array(&m["emissiveFactor"], [0.; 3]);
        let strength = m["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"]
            .as_f64()
            .unwrap_or(1.) as f32;
        out.pbr.emissive_roughness = [
            emission[0] * strength,
            emission[1] * strength,
            emission[2] * strength,
            p["roughnessFactor"].as_f64().unwrap_or(1.) as f32,
        ];
        out.pbr.surface = [
            p["metallicFactor"].as_f64().unwrap_or(1.) as f32,
            m["normalTexture"]["scale"].as_f64().unwrap_or(1.) as f32,
            1.,
            m["alphaCutoff"].as_f64().unwrap_or(0.5) as f32,
        ];
        out.pbr.textures = [
            texture(&p["baseColorTexture"], "color")?,
            texture(&m["normalTexture"], "normal")?,
            texture(&p["metallicRoughnessTexture"], "linear")?,
            texture(&m["emissiveTexture"], "color")?,
        ];
        out.extras.occlusion_texture = texture(&m["occlusionTexture"], "linear")?;
        out.extras.occlusion_strength =
            m["occlusionTexture"]["strength"].as_f64().unwrap_or(1.) as f32;
        out.pbr.flags[0] = match m["alphaMode"].as_str() {
            Some("MASK") => 1,
            Some("BLEND") => 2,
            _ => 0,
        };
        out.pbr.flags[1] = ara_render::scene::VERTEX_COLOR_TINT
            | if m["doubleSided"] == true {
                ara_render::scene::DOUBLE_SIDED
            } else {
                0
            };
        out.extras.volume = Volume {
            ior: m["extensions"]["KHR_materials_ior"]["ior"]
                .as_f64()
                .unwrap_or(1.5) as f32,
            ..Default::default()
        }
        .pack();
        scene.materials.push(out);
    }
    let mut meshes = Vec::new();
    for mesh in doc["meshes"].as_array().context("Polar meshes")? {
        let mut entries = Vec::new();
        for p in mesh["primitives"].as_array().context("Polar primitives")? {
            ensure!(
                p["mode"].as_u64().unwrap_or(4) == 4,
                "Polar triangles required"
            );
            let a = &p["attributes"];
            let pos = floats::<3>(doc, &bin, &a["POSITION"])?;
            let normal = floats::<3>(doc, &bin, &a["NORMAL"])?;
            let uv = floats::<2>(doc, &bin, &a["TEXCOORD_0"])?;
            let uv1 = if a["TEXCOORD_1"].is_null() {
                None
            } else {
                Some(floats::<2>(doc, &bin, &a["TEXCOORD_1"])?)
            };
            let tangent = floats::<4>(doc, &bin, &a["TANGENT"])?;
            let colors = if a["COLOR_0"].is_null() {
                None
            } else {
                Some(floats::<4>(doc, &bin, &a["COLOR_0"])?)
            };
            ensure!(
                normal.len() == pos.len()
                    && uv.len() == pos.len()
                    && tangent.len() == pos.len()
                    && uv1.as_ref().is_none_or(|v| v.len() == pos.len())
                    && colors.as_ref().is_none_or(|v| v.len() == pos.len()),
                "Polar vertex count mismatch"
            );
            let mut bounds = [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)];
            let vertices = pos
                .iter()
                .enumerate()
                .map(|(i, &position)| {
                    bounds[0] = bounds[0].min(Vec3::from(position));
                    bounds[1] = bounds[1].max(Vec3::from(position));
                    Vertex {
                        position,
                        normal: normal[i],
                        uv: uv[i],
                        uv1: uv1.as_ref().map_or(uv[i], |v| v[i]),
                        tangent: tangent[i],
                        color: colors.as_ref().map_or(u32::MAX, |v| pack_color(v[i])),
                        terrain_weights: 0,
                    }
                })
                .collect();
            let a = &doc["accessors"][index(&p["indices"])?];
            let view = &doc["bufferViews"][index(&a["bufferView"])?];
            let start =
                index(&view["byteOffset"]).unwrap_or(0) + index(&a["byteOffset"]).unwrap_or(0);
            let count = index(&a["count"])?;
            let bytes = match a["componentType"].as_u64() {
                Some(5121) => 1,
                Some(5123) => 2,
                Some(5125) => 4,
                _ => anyhow::bail!("Polar index type {}", a["componentType"]),
            };
            ensure!(start + count * bytes <= bin.len(), "Polar index bounds");
            let indices = bin[start..start + count * bytes]
                .chunks_exact(bytes)
                .map(|p| {
                    if bytes == 1 {
                        u32::from(p[0])
                    } else if bytes == 2 {
                        u32::from(u16::from_le_bytes([p[0], p[1]]))
                    } else {
                        u32::from_le_bytes([p[0], p[1], p[2], p[3]])
                    }
                })
                .collect();
            let mut digest = Sha256::new();
            digest.update(&bin);
            digest.update(scene.geometry.len().to_le_bytes());
            entries.push((scene.geometry.len(), index(&p["material"])?));
            scene.geometry.push(Geometry {
                key: digest.finalize().into(),
                vertices,
                lods: vec![Lod { indices, error: 0. }],
                bounds,
                deforming: false,
                vertex_colors: colors.is_some(),
                revision: 0,
            });
        }
        meshes.push(entries);
    }
    let roots = doc["scenes"][index(&doc["scene"]).unwrap_or(0)]["nodes"]
        .as_array()
        .context("Polar roots")?;
    let mut pending = roots
        .iter()
        .map(|n| Ok((index(n)?, Mat4::IDENTITY)))
        .collect::<Result<Vec<_>>>()?;
    while let Some((id, parent)) = pending.pop() {
        let n = &doc["nodes"][id];
        let local = if n["matrix"].is_null() {
            Mat4::from_scale_rotation_translation(
                Vec3::from(array(&n["scale"], [1.; 3])),
                Quat::from_array(array(&n["rotation"], [0., 0., 0., 1.])),
                Vec3::from(array(&n["translation"], [0.; 3])),
            )
        } else {
            Mat4::from_cols_array(&array(&n["matrix"], [0.; 16]))
        };
        let world = parent * local;
        if let Some(children) = n["children"].as_array() {
            for c in children {
                pending.push((index(c)?, world));
            }
        }
        if let Some(m) = n["mesh"].as_u64() {
            for &(geometry, material) in &meshes[m as usize] {
                let entity = scene.instances.len() as u64 + 1;
                scene.instances.push(Instance {
                    entity,
                    name: n["name"].as_str().unwrap_or("Polar").into(),
                    geometry,
                    material,
                    world,
                    previous_world: world,
                    casts_shadows: true,
                    receives_decals: true,
                    paint: None,
                });
            }
        }
    }
    // Controlled finite-range lab lights, deliberately separate from the editor's
    // fog/post/sky settings: both renderers receive this identical scene snapshot.
    for z in [4., 0., -4., -8.] {
        scene.lights.push(Light {
            position_range: [-2.5, 3., z, 8.],
            color_lumens: [1., 0.85, 0.65, 1500.],
            parameters: [0., 1., 0.1, 1.],
            direction_outer: [0., -1., 0., -1.],
            ..Light::ZERO
        });
    }
    scene.validate()?;
    Ok(Fixture {
        scene,
        camera: CameraSpec::LookAt {
            position: Vec3::new(-2.54, 1.98, 7.18),
            target: Vec3::new(-2.54, 0.5, -3.),
            vertical_fov: 58.72_f32.to_radians(),
        },
        extent: 20.,
        ev100: 3.,
        textures,
    })
}
