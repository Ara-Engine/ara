//! Unity comparison export (measurement tooling; nothing ships it).
//!
//! With `ARA_UNITY_EXPORT=<dir>` the bridge writes the scene it hands Renderlab, once, as
//! `<dir>/scene.gltf` (+ `.bin`, `textures/`) and a `<dir>/scene.json` sidecar for what glTF
//! cannot carry (sun, sky, fog, exposure, light units, material classes, the camera clip).
//! The frame is the `ARA_UNITY_EXPORT_FRAME`th rendered one (default 3, after the capture's
//! content has resolved). `ARA_UNITY_EXPORT_CLIP=frames,mx,my,mz,yawDeg` records the
//! `ara capture --video` move the Unity players replay (`ara-player/src/world/video.rs`).
//! Skinned draws are baked at the exported frame's pose; terrain tiles and foliage are what
//! Renderlab draws on the RT path (`Terrain::foliage`). Layout and fidelity limits are in the
//! sidecar's `notes`.
use crate::{gaps::Gaps, lighting, unity_export_mesh::normalize, unity_gltf::Writer};
use anyhow::{Context, Result};
use ara_runtime::{
    extract::{Draw, Geometry, RenderFrame},
    model::{ModelMaterial, ModelTexture},
};
use ara_runtime_render::{ExternalView, terrain_mesh::TerrainMeshes};
use glam34::{Quat, Vec3};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        OnceLock,
        atomic::{AtomicU32, Ordering},
    },
};

static CALLS: AtomicU32 = AtomicU32::new(0);
static TARGET: OnceLock<Option<(PathBuf, u32)>> = OnceLock::new();

/// Called by the bridge once per frame after the scene sync, with the foliage draws it derived.
/// Exports on the configured frame; free otherwise.
pub(crate) fn on_frame(view: &ExternalView<'_>, foliage: &[Draw]) -> Result<()> {
    let target = TARGET.get_or_init(|| {
        let dir = std::env::var_os("ARA_UNITY_EXPORT").filter(|v| !v.is_empty())?;
        let at = std::env::var("ARA_UNITY_EXPORT_FRAME")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        Some((dir.into(), at))
    });
    let Some((dir, at)) = target else {
        return Ok(());
    };
    if CALLS.fetch_add(1, Ordering::Relaxed) + 1 != *at {
        return Ok(());
    }
    export(view, foliage, dir).context("ARA_UNITY_EXPORT")
}

/// A decoded cooked texture: RGBA8 of the finest level.
struct Image {
    size: [u32; 2],
    rgba: Vec<u8>,
}

/// What one export has built so far.
#[derive(Default)]
pub(crate) struct Export {
    pub(crate) meshes: HashMap<String, usize>,
    materials: HashMap<String, usize>,
    /// Material records for the sidecar, in glTF material order.
    records: Vec<Value>,
    decoded: HashMap<(usize, usize), Rc<Image>>,
    files: HashMap<String, usize>,
    notes: Vec<String>,
    triangles: u64,
    nodes: u64,
    skinned: Vec<String>,
    /// Node names (`label#n`) of draws that block no light.
    no_shadow: Vec<String>,
    glass: Vec<String>,
    uv1_materials: u32,
    transformed_materials: u32,
}

fn export(view: &ExternalView<'_>, foliage: &[Draw], dir: &Path) -> Result<()> {
    let started = std::time::Instant::now();
    std::fs::create_dir_all(dir.join("textures"))?;
    let frame = view.frame;
    let mut gaps = Gaps::default();
    let mut w = Writer::default();
    let mut x = Export::default();
    for draw in frame.draws.iter() {
        x.draw(&mut w, frame, draw, false, dir)?;
    }
    let entity_nodes = x.nodes;
    for draw in foliage {
        x.draw(&mut w, frame, draw, true, dir)?;
    }
    let foliage_nodes = x.nodes - entity_nodes;
    let terrain = x.terrain(&mut w, view, dir)?;
    let lights = x.lights(&mut w, frame);
    let sky = sky(view, &mut gaps, dir)?;
    let camera = frame.camera.context("the frame has no camera")?;
    let clip = std::env::var("ARA_UNITY_EXPORT_CLIP")
        .ok()
        .map(|c| c.split(',').filter_map(|v| v.trim().parse::<f32>().ok()).collect::<Vec<_>>())
        .filter(|c| c.len() == 5)
        .map(|c| json!({"frames": c[0] as u32, "travel": [c[1], c[2], c[3]], "yawDegrees": c[4],
            "warmupFrames": 30, "framesPerSecond": 60,
            "note": "ara-player/src/world/video.rs: t = max(i-30,0)/(frames-1); position = start + travel*t; forward rotated about +Y by yaw*t: [x cos + z sin, y, z cos - x sin]"}));
    let (sun, other_light) = sun(frame);
    let fog = frame
        .look
        .fog
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?;
    let atmosphere = frame
        .look
        .sky
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?;
    let environment = serde_json::to_value(frame.environment)?;
    let sidecar = json!({
        "format": "ara-unity-export/1",
        "units": {
            "length": "metres", "coordinates": "right-handed, +Y up, camera looks down local -Z (glTF); Unity builds negate Z",
            "illuminance": "lux (sun lux is linear RGB, as Ara authors it)",
            "luminance": "cd/m2 (emissive nits, sky radiance)",
            "localLights": "Ara stores point-equivalent lumens = 4*pi*candela; the glTF lights carry candela (KHR_lights_punctual), `lights[].lumens` is Ara's value",
        },
        "camera": {
            "position": camera.position, "forward": camera.forward, "verticalFovDegrees": camera.fov,
            "near": 0.05, "far": 4000.0, "clip": clip,
        },
        "exposure": {
            "ev100Lock": view.look.ev100, "compensation": view.look.compensation,
            "legacyLinearExposure": frame.environment.exposure,
            "note": "Renderlab's bridge meters the shown image to middle grey when ev100Lock is null; captures lock it with --ev100.",
        },
        "display": {
            "toneMapper": format!("{:?}", view.look.tone_mapper), "bloom": view.look.bloom,
            "grading": format!("{:?}", view.look.grading), "frameEnvironment": format!("{:?}", view.look),
        },
        "sun": sun,
        "otherAtmosphereLight": other_light,
        "environment": environment,
        "sky": sky,
        "skyAtmosphere": atmosphere,
        "heightFog": fog,
        "lights": lights,
        "materials": x.records,
        "terrain": terrain,
        "stats": {
            "entityNodes": entity_nodes, "foliageNodes": foliage_nodes,
            "triangles": x.triangles, "uniqueMeshes": x.meshes.len(), "materials": x.materials.len(),
            "effects": frame.effects.len(), "decals": frame.decals.len(),
            "reflectionProbes": frame.reflection_probes.len(),
            "materialsWithUv1": x.uv1_materials, "materialsWithTextureTransform": x.transformed_materials,
            "exportSeconds": started.elapsed().as_secs_f32(),
        },
        "skinnedDraws": x.skinned,
        "noShadowNodes": x.no_shadow,
        "glassDraws": x.glass,
        "shadows": "Ara's Light component has no cast-shadows flag: the sun and every local light are shadowed by the renderer; every draw casts unless `extras.castsShadows` is false.",
        "notes": x.notes,
    });
    w.write(dir, "scene")?;
    std::fs::write(dir.join("scene.json"), serde_json::to_vec_pretty(&sidecar)?)?;
    eprintln!(
        "ARA_UNITY_EXPORT: wrote {} ({} nodes, {} triangles, {} materials) in {:.1}s",
        dir.display(),
        x.nodes,
        x.triangles,
        x.materials.len(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// The draw's material and the textures its indices address, as the scene sync resolves them.
fn source(draw: &Draw) -> Option<(&ModelMaterial, &[ModelTexture])> {
    if let Some(native) = draw.material.as_deref() {
        return Some((&native.material, &native.textures));
    }
    let Geometry::Model { model, primitive } = &draw.geometry else {
        return None;
    };
    let slot = model.primitives.get(*primitive)?.material?;
    Some((model.materials.get(slot)?, &model.textures))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// A rotation turning local -Z (where glTF lights shine) onto `travel`.
fn aim(travel: [f32; 3]) -> [f32; 4] {
    let to = Vec3::from_array(travel).normalize_or(Vec3::NEG_Z);
    Quat::from_rotation_arc(Vec3::NEG_Z, to).to_array()
}

impl Export {
    /// Decodes a cooked texture once.
    fn image(&mut self, texture: &ModelTexture) -> Result<Rc<Image>> {
        let key = (texture.ktx2.as_ptr() as usize, texture.ktx2.len());
        if let Some(done) = self.decoded.get(&key) {
            return Ok(done.clone());
        }
        let decoded = ara_runtime_render::decode_texture(&texture.ktx2)
            .with_context(|| format!("decoding {}", texture.name))?;
        let image = Rc::new(Image {
            size: decoded.size,
            rgba: decoded.rgba,
        });
        // Decoded 4096 maps are 64 MB each: keep only the recent few.
        if self.decoded.len() > 24 {
            self.decoded.clear();
        }
        self.decoded.insert(key, image.clone());
        Ok(image)
    }

    /// Writes `rgba` as `textures/<name>.png` (RGB when fully opaque) and returns its glTF
    /// texture index, shared by `name`.
    fn file(
        &mut self,
        w: &mut Writer,
        dir: &Path,
        name: &str,
        size: [u32; 2],
        rgba: &[u8],
        clamp: bool,
    ) -> Result<usize> {
        if let Some(index) = self.files.get(name) {
            return Ok(*index);
        }
        let path = dir.join("textures").join(format!("{name}.png"));
        if rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
            let rgb: Vec<u8> = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            image::save_buffer(&path, &rgb, size[0], size[1], image::ColorType::Rgb8)?;
        } else {
            image::save_buffer(&path, rgba, size[0], size[1], image::ColorType::Rgba8)?;
        }
        let index = w.texture(&format!("textures/{name}.png"), clamp);
        self.files.insert(name.to_owned(), index);
        Ok(index)
    }

    /// One draw as a glTF node (entity draws and foliage parts alike).
    fn draw(
        &mut self,
        w: &mut Writer,
        frame: &RenderFrame,
        draw: &Draw,
        foliage: bool,
        dir: &Path,
    ) -> Result<()> {
        if !draw.world.is_finite() || draw.world.determinant() == 0. {
            return Ok(());
        }
        let (material, with_tangents) = self.material(w, draw, dir)?;
        let mesh = self.mesh(w, frame, draw, material, with_tangents)?;
        let triangles = match &draw.geometry {
            Geometry::Model { model, primitive } => model.primitives[*primitive].indices.len() / 3,
            Geometry::Shape { .. } => 0,
        };
        self.triangles += triangles as u64;
        self.nodes += 1;
        if draw.skin.is_some() {
            self.skinned.push(draw.address.clone());
        }
        let name = format!(
            "{}#{}",
            if foliage {
                "foliage"
            } else {
                draw.label.as_str()
            },
            self.nodes
        );
        if !draw.casts_shadows {
            self.no_shadow.push(name.clone());
        }
        w.node(json!({
            "name": name,
            "mesh": mesh,
            "matrix": draw.world.to_cols_array(),
            "extras": {"address": draw.address, "castsShadows": draw.casts_shadows, "skinnedAtExportPose": draw.skin.is_some(), "foliage": foliage},
        }));
        Ok(())
    }

    /// The glTF material of a draw (shared by content) and whether its mesh may carry tangents.
    fn material(&mut self, w: &mut Writer, draw: &Draw, dir: &Path) -> Result<(usize, bool)> {
        let src = source(draw);
        let mut key = format!(
            "{:?}{:?}{:?}{:?}{:?}",
            draw.color.map(f32::to_bits),
            draw.roughness.to_bits(),
            draw.metallic.to_bits(),
            draw.emissive.map(f32::to_bits),
            draw.emissive_factor.map(|f| f.map(f32::to_bits))
        );
        match (&draw.material, &draw.geometry) {
            (Some(native), _) => key += &format!("native{}", hex(&native.digest.0)),
            (None, Geometry::Model { model, primitive }) => {
                let slot = model.primitives.get(*primitive).and_then(|p| p.material);
                key += &format!("model{}:{slot:?}", hex(&model.digest.0));
            }
            _ => {}
        }
        if let Some(index) = self.materials.get(&key) {
            return Ok((*index, src.is_some_and(|(m, _)| m.normal_texture.is_some())));
        }
        let id = self.records.len();
        let mut base = draw.color;
        let (mut metallic, mut roughness) = (draw.metallic, draw.roughness);
        let mut glow = match &draw.geometry {
            Geometry::Shape { .. } => draw.emissive,
            Geometry::Model { .. } => {
                let f = draw.emissive_factor.unwrap_or([0.; 3]);
                std::array::from_fn(|i| f[i] * draw.emissive[i])
            }
        };
        let mut pbr = json!({});
        let mut record = json!({"index": id, "name": format!("mat{id}")});
        let mut extensions: Vec<&'static str> = Vec::new();
        let mut material = json!({"name": format!("mat{id}"), "doubleSided": true});
        let mut tangents = false;
        let mut alpha = "OPAQUE";
        let mut cutoff = 0.5f32;
        if let Some((m, textures)) = src {
            for (out, (a, b)) in base.iter_mut().zip(m.base_color.iter().zip(draw.color)) {
                *out = a * b;
            }
            metallic = m.metallic;
            roughness = m.roughness;
            let factor = draw.emissive_factor.unwrap_or(m.emissive);
            glow = std::array::from_fn(|i| factor[i] * draw.emissive[i]);
            material["doubleSided"] = json!(m.double_sided);
            record["sourceName"] = json!(m.name);
            let opacity = base_alpha(m, draw);
            let uv = [
                m.base_color_uv,
                m.normal_uv,
                m.metallic_roughness_uv,
                m.emissive_uv,
            ];
            if uv.iter().any(|t| t.tex_coord != 0) {
                self.uv1_materials += 1;
            }
            if uv
                .iter()
                .any(|t| t.offset != [0.; 2] || t.scale != [1.; 2] || t.rotation != 0.)
            {
                self.transformed_materials += 1;
            }
            let transform = |t: &ara_runtime::model::TextureTransform| {
                (t.offset != [0.; 2] || t.scale != [1.; 2] || t.rotation != 0.).then(|| {
                    json!({"KHR_texture_transform": {"offset": t.offset, "scale": t.scale, "rotation": t.rotation}})
                })
            };
            let info = |index: usize, t: &ara_runtime::model::TextureTransform, extra: Value| {
                let mut v = json!({"index": index});
                if let (Some(ext), Some(obj)) = (transform(t), v.as_object_mut()) {
                    obj.insert("extensions".into(), ext);
                }
                if let (Some(extra), Some(obj)) = (extra.as_object(), v.as_object_mut()) {
                    obj.extend(extra.clone());
                }
                v
            };
            record["uv"] = json!({"baseColor": m.base_color_uv, "normal": m.normal_uv, "metallicRoughness": m.metallic_roughness_uv, "emissive": m.emissive_uv});
            let mut alpha_class = "opaque";
            if let Some(texture) = m.base_color_texture.and_then(|i| textures.get(i)) {
                let image = self.image(texture)?;
                let name = format!("c_{}", hex(&blake(&texture.ktx2)));
                let index = self.file(w, dir, &name, image.size, &image.rgba, false)?;
                pbr["baseColorTexture"] = info(index, &m.base_color_uv, json!({}));
                record["baseColorFile"] = json!(format!("textures/{name}.png"));
                let (min, partial) = alpha_stats(&image.rgba);
                alpha_class = if min >= 250 {
                    "opaque"
                } else if partial < 0.15 {
                    "binary"
                } else {
                    "partial"
                };
            }
            // The blend decision the bridge makes: see-through only when it is seen through.
            match m.alpha_mode {
                1 => {
                    alpha = "MASK";
                    cutoff = m.alpha_cutoff;
                }
                2 if opacity < 1. || alpha_class == "partial" => alpha = "BLEND",
                2 if alpha_class == "binary" => alpha = "MASK",
                _ => {}
            }
            if m.base_color_texture.is_some() && alpha == "OPAQUE" {
                base[3] = 1.;
            }
            record["alphaClass"] = json!(alpha_class);
            // Metallic-roughness: baked as Unity's mask map (R metallic, G 1, B 0, A smoothness)
            // with the factors folded in, so both pipelines read it unscaled.
            if let Some(texture) = m.metallic_roughness_texture.and_then(|i| textures.get(i)) {
                let image = self.image(texture)?;
                let name = format!(
                    "mr_{}_{:x}_{:x}",
                    hex(&blake(&texture.ktx2)),
                    m.metallic.to_bits(),
                    m.roughness.to_bits()
                );
                let mask: Vec<u8> = image
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| {
                        let metal = f32::from(p[2]) / 255. * m.metallic;
                        let smooth = 1. - f32::from(p[1]) / 255. * m.roughness;
                        [
                            (metal.clamp(0., 1.) * 255.).round() as u8,
                            255,
                            0,
                            (smooth.clamp(0., 1.) * 255.).round() as u8,
                        ]
                    })
                    .collect();
                let index = self.file(w, dir, &name, image.size, &mask, false)?;
                record["maskMapFile"] = json!(format!("textures/{name}.png"));
                record["maskMapNote"] = json!(
                    "R metallic (factor folded in), G 1, B 0, A smoothness = 1 - roughness*roughnessFactor; source was glTF metallicRoughness (G rough, B metal)"
                );
                // glTF's own slot, for readers that import the original: the untouched map.
                let _ = index;
            }
            if let Some(texture) = m.normal_texture.and_then(|i| textures.get(i)) {
                let image = self.image(texture)?;
                let name = format!("n_{}", hex(&blake(&texture.ktx2)));
                let index = self.file(w, dir, &name, image.size, &image.rgba, false)?;
                material["normalTexture"] =
                    info(index, &m.normal_uv, json!({"scale": m.normal_scale}));
                record["normalFile"] = json!(format!("textures/{name}.png"));
                record["normalScale"] = json!(m.normal_scale);
                tangents = true;
            }
            if let Some(texture) = m.emissive_texture.and_then(|i| textures.get(i)) {
                let image = self.image(texture)?;
                let name = format!("e_{}", hex(&blake(&texture.ktx2)));
                let index = self.file(w, dir, &name, image.size, &image.rgba, false)?;
                material["emissiveTexture"] = info(index, &m.emissive_uv, json!({}));
                record["emissiveFile"] = json!(format!("textures/{name}.png"));
            }
            if m.occlusion_texture.is_some() {
                self.note("ambient-occlusion maps are not exported (Renderlab ignores them too: occlusion comes from traced GI)");
            }
            let transmission = m.transmission_factor * (1. - m.metallic).clamp(0., 1.);
            if m.transmission_factor > 0. {
                record["glass"] = json!({
                    "transmission": m.transmission_factor, "effectiveTransmission": transmission,
                    "ior": m.volume.ior, "thickness": m.volume.thickness,
                    "attenuationColor": m.volume.attenuation_color, "attenuationDistance": m.volume.attenuation_distance,
                    "variesOverSurface": m.transmission_texture.is_some() || (m.metallic > 0. && m.metallic_roughness_texture.is_some()),
                });
                self.glass.push(draw.address.clone());
                material["extensions"] = json!({
                    "KHR_materials_transmission": {"transmissionFactor": m.transmission_factor},
                    "KHR_materials_ior": {"ior": m.volume.ior},
                });
                extensions.extend(["KHR_materials_transmission", "KHR_materials_ior"]);
                if m.volume.thickness > 0. && m.volume.attenuation_distance.is_finite() {
                    material["extensions"]["KHR_materials_volume"] = json!({
                        "thicknessFactor": m.volume.thickness,
                        "attenuationColor": m.volume.attenuation_color,
                        "attenuationDistance": m.volume.attenuation_distance,
                    });
                    extensions.push("KHR_materials_volume");
                }
            }
            if draw.material.is_some() {
                self.note("native material-graph materials are exported as their base factors and maps (as Renderlab draws them)");
            }
        }
        pbr["baseColorFactor"] = json!(base);
        pbr["metallicFactor"] = json!(metallic);
        pbr["roughnessFactor"] = json!(roughness);
        material["pbrMetallicRoughness"] = pbr;
        material["alphaMode"] = json!(alpha);
        if alpha == "MASK" {
            material["alphaCutoff"] = json!(cutoff);
        }
        let peak = glow.iter().copied().fold(0f32, f32::max);
        if peak > 0. {
            material["emissiveFactor"] = json!(glow.map(|c| (c / peak).clamp(0., 1.)));
            material["extensions"]["KHR_materials_emissive_strength"] =
                json!({"emissiveStrength": peak});
            extensions.push("KHR_materials_emissive_strength");
        }
        record["baseColor"] = json!(base);
        record["metallic"] = json!(metallic);
        record["roughness"] = json!(roughness);
        record["emissiveNits"] = json!(glow);
        record["alphaMode"] = json!(alpha);
        record["alphaCutoff"] = json!(cutoff);
        record["doubleSided"] = material["doubleSided"].clone();
        let index = w.material(material, &extensions);
        debug_assert_eq!(index, id);
        self.records.push(record);
        self.materials.insert(key, index);
        Ok((index, tangents))
    }

    fn note(&mut self, text: &str) {
        if !self.notes.iter().any(|n| n == text) {
            self.notes.push(text.to_owned());
        }
    }

    /// Terrain tiles as world-space meshes with Ara's splat-coloured base colour (one texel
    /// per height sample). The layers' detail and normal maps are not exported.
    fn terrain(&mut self, w: &mut Writer, view: &ExternalView<'_>, dir: &Path) -> Result<Value> {
        let Some(terrain) = view.frame.terrain.as_ref() else {
            return Ok(Value::Null);
        };
        let mut coords = terrain.tiles.clone();
        coords.sort();
        coords.dedup();
        let layers = &terrain.data.header.materials;
        let resolved: Vec<f32> = layers
            .iter()
            .filter_map(|r| view.terrain_materials.get(&r.0))
            .map(|a| a.material.roughness)
            .collect();
        let roughness = if resolved.is_empty() {
            0.5
        } else {
            (resolved.iter().sum::<f32>() / resolved.len() as f32).clamp(0.045, 1.)
        };
        // Each layer's colour: its base colour factor times the mean colour of its base-colour map
        // (Renderlab flattens the maps against their mean the same way), so sand reads as sand.
        let mut layer_colours = [[1f32; 3]; 4];
        for (colour, reference) in layer_colours.iter_mut().zip(layers.iter()) {
            let Some(asset) = view.terrain_materials.get(&reference.0) else {
                continue;
            };
            let m = &asset.material;
            *colour = [m.base_color[0], m.base_color[1], m.base_color[2]];
            if let Some(texture) = m.base_color_texture.and_then(|i| asset.textures.get(i)) {
                let image = self.image(texture)?;
                let mean = crate::texture::mean_linear(&image.rgba);
                for k in 0..3 {
                    colour[k] *= mean[k];
                }
            }
        }
        let mut meshes = TerrainMeshes::default();
        let mut tiles = 0;
        for coord in &coords {
            let Some(tile) = meshes.tile(&terrain.data, *coord) else {
                continue;
            };
            let name = format!("terrain_{}_{}", coord.x, coord.z);
            let weights = terrain
                .data
                .tiles
                .get(coord)
                .map(|t| &t.weights[..])
                .unwrap_or(&[]);
            let mut rgba = tile.texture.rgba.clone();
            if weights.len() * 4 == rgba.len() {
                for (texel, weight) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(weights) {
                    let total = weight.iter().map(|w| f32::from(*w)).sum::<f32>().max(1.);
                    for k in 0..3 {
                        let linear: f32 = layer_colours
                            .iter()
                            .zip(weight)
                            .map(|(c, w)| c[k] * f32::from(*w) / total)
                            .sum();
                        texel[k] = crate::texture::linear_to_srgb(linear);
                    }
                }
            }
            let index = self.file(w, dir, &name, tile.texture.size, &rgba, true)?;
            let material = w.material(json!({
                "name": name, "doubleSided": true,
                "pbrMetallicRoughness": {"baseColorFactor": [1, 1, 1, 1], "metallicFactor": 0.0, "roughnessFactor": roughness,
                    "baseColorTexture": {"index": index}},
            }), &[]);
            self.records.push(json!({"index": material, "name": name, "terrain": true, "baseColorFile": format!("textures/{name}.png"), "roughness": roughness, "metallic": 0.0, "baseColor": [1, 1, 1, 1], "alphaMode": "OPAQUE", "doubleSided": true, "emissiveNits": [0, 0, 0]}));
            let vertices = &tile.source.vertices;
            let mut positions = Vec::with_capacity(vertices.len() * 3);
            let mut normals = Vec::with_capacity(vertices.len() * 3);
            let mut uvs = Vec::with_capacity(vertices.len() * 2);
            for v in vertices {
                positions.extend_from_slice(&v.position);
                normals.extend_from_slice(&normalize(v.normal));
                uvs.extend_from_slice(&v.uv);
            }
            let attributes = json!({
                "POSITION": w.floats(&positions, 3, true),
                "NORMAL": w.floats(&normals, 3, false),
                "TEXCOORD_0": w.floats(&uvs, 2, false),
            });
            let indices = w.indices(&tile.source.indices);
            let mesh = w.mesh(&name, attributes, indices, material);
            w.node(json!({"name": name, "mesh": mesh, "extras": {"terrain": true, "castsShadows": true}}));
            self.triangles += (tile.source.indices.len() / 3) as u64;
            self.nodes += 1;
            tiles += 1;
        }
        self.note("terrain tiles carry one colour per height sample: the splat weights blending each layer's base colour times the mean of its colour map; the layers' detail, normal and roughness maps and the side projections are not exported");
        let h = &terrain.data.header;
        Ok(json!({
            "residentTiles": tiles, "origin": h.origin, "spacing": h.spacing, "cells": h.cells,
            "layers": layers.len(), "roughness": roughness,
            "foliageInstances": terrain.data.foliage.len(),
        }))
    }

    /// Local lights as glTF punctual lights (candela) with Ara's own values beside them.
    fn lights(&mut self, w: &mut Writer, frame: &RenderFrame) -> Vec<Value> {
        use ara_runtime::components::LightKind;
        let mut out = Vec::new();
        if let Some((toward, lux)) = frame.sun {
            let peak = lux.iter().copied().fold(0f32, f32::max).max(1e-6);
            let light = w.light(json!({"type": "directional", "name": "sun", "color": lux.map(|c| c / peak), "intensity": peak}));
            w.node(json!({"name": "sun", "rotation": aim(toward.map(|c| -c)), "extensions": {"KHR_lights_punctual": {"light": light}}}));
        }
        for (i, l) in frame.lights.iter().enumerate() {
            let candela = l.lumens.max(0.) / (4. * std::f32::consts::PI);
            let (kind, spot) = match l.kind {
                LightKind::Spot => ("spot", Some((l.cone[0], l.cone[1]))),
                LightKind::Rect | LightKind::Disc => ("spot", Some((0., 88.))),
                _ => ("point", None),
            };
            let mut light = json!({"type": kind, "name": format!("light{i}"), "color": l.color, "intensity": candela});
            if l.range > 0. {
                light["range"] = json!(l.range);
            }
            if let Some((inner, outer)) = spot {
                let outer = outer.to_radians().clamp(0.01, 1.55);
                light["spot"] = json!({"innerConeAngle": inner.to_radians().clamp(0., outer * 0.999), "outerConeAngle": outer});
            }
            let index = w.light(light);
            let mut node = json!({"name": format!("light{i}"), "translation": l.position, "extensions": {"KHR_lights_punctual": {"light": index}}});
            if spot.is_some() {
                node["rotation"] = json!(aim(l.direction));
            }
            w.node(node);
            out.push(json!({
                "index": i, "kind": format!("{:?}", l.kind), "position": l.position, "direction": l.direction,
                "color": l.color, "lumens": l.lumens, "candela": candela, "range": l.range,
                "coneDegrees": l.cone, "sourceRadius": l.radius, "sourceLength": l.length, "rectSize": l.size,
                "volumetric": l.volumetric, "iesProfile": l.profile.is_some(),
            }));
        }
        out
    }
}

/// The alpha a material and draw give together (factor times factor).
fn base_alpha(m: &ModelMaterial, draw: &Draw) -> f32 {
    m.base_color[3] * draw.color[3]
}

/// The smallest alpha of an RGBA8 image and the share of texels that are partly there.
fn alpha_stats(rgba: &[u8]) -> (u8, f32) {
    let mut min = 255u8;
    let mut partial = 0usize;
    for p in rgba.as_chunks::<4>().0 {
        min = min.min(p[3]);
        if (8..248).contains(&p[3]) {
            partial += 1;
        }
    }
    (min, partial as f32 / (rgba.len() / 4).max(1) as f32)
}

/// A short stable hash of bytes (FNV-1a over a sample plus the length), for file names.
fn blake(bytes: &[u8]) -> [u8; 8] {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let stride = (bytes.len() / 65_536).max(1);
    for b in bytes
        .iter()
        .step_by(stride)
        .chain(bytes.len().to_le_bytes().iter())
    {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h.to_be_bytes()
}

/// The sun (and a second atmosphere light) as the sidecar records them.
fn sun(frame: &RenderFrame) -> (Value, Value) {
    let sun = frame.sun.map(|(toward, lux)| json!({
        "directionTowardLight": toward, "luxRgb": lux, "angularRadiusRadians": frame.sun_radius,
        "atmosphere": frame.sun_atmosphere.map(|a| json!({"discScale": a.disc_scale, "perPixelTransmittance": a.per_pixel_transmittance})),
        "atmosphereIndex": frame.sun_atmosphere_index, "volumetric": frame.sun_volumetric,
        "castsShadows": true,
    }));
    let other = frame.other_atmosphere_light.map(|d| json!({
        "directionTowardLight": d.direction, "luxRgb": d.lux, "angularRadiusRadians": d.radius, "volumetric": d.volumetric,
    }));
    (sun.unwrap_or(Value::Null), other.unwrap_or(Value::Null))
}

/// The sky: a physical atmosphere (the sidecar's `skyAtmosphere`) or an equirectangular
/// radiance map, written as Radiance RGBE (`sky.hdr`, cd/m2).
fn sky(view: &ExternalView<'_>, gaps: &mut Gaps, dir: &Path) -> Result<Value> {
    let light = lighting::sky_light(view);
    Ok(match lighting::sky(view, gaps) {
        lighting::SkySource::Atmosphere(_) => json!({"kind": "atmosphere", "skyLight": light}),
        lighting::SkySource::Map { size, pixels } => {
            // The gradient sky is 64 wide by 128 tall (azimuth by elevation): repeat columns to the
            // 2:1 equirectangular shape a Unity cubemap import wants.
            let (size, pixels) = if size[0] < size[1] {
                let wide = size[1] * 2;
                let widened: Vec<[f32; 4]> = (0..size[1])
                    .flat_map(|y| (0..wide).map(move |x| (x, y)))
                    .map(|(x, y)| pixels[(y * size[0] + x * size[0] / wide) as usize])
                    .collect();
                ([wide, size[1]], std::borrow::Cow::Owned(widened))
            } else {
                (size, pixels)
            };
            write_hdr(&dir.join("sky.hdr"), size, &pixels)?;
            json!({"kind": "map", "file": "sky.hdr", "size": size, "skyLight": light,
                "convention": "equirectangular radiance in cd/m2, row 0 the zenith; the orientation Renderlab's Sky::Map takes"})
        }
    })
}

/// Writes flat (uncompressed) Radiance RGBE.
fn write_hdr(path: &Path, size: [u32; 2], pixels: &[[f32; 4]]) -> Result<()> {
    let mut out = format!(
        "#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {} +X {}\n",
        size[1], size[0]
    )
    .into_bytes();
    out.reserve(pixels.len() * 4);
    for p in pixels {
        let v = p[0].max(p[1]).max(p[2]);
        if !v.is_finite() || v < 1e-32 {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let e = v.log2().floor() as i32 + 1;
        let scale = 256. / 2f32.powi(e);
        out.extend_from_slice(&[
            (p[0] * scale) as u8,
            (p[1] * scale) as u8,
            (p[2] * scale) as u8,
            (e + 128) as u8,
        ]);
    }
    std::fs::write(path, out)?;
    Ok(())
}
