//! Presents `ara_runtime::extract::RenderFrame`s with `ara-render`.
//!
//! The renderer uploads a scene snapshot once; [`Presenter`] rebuilds it only when the
//! set of draws, geometry or materials changes (spawns, asset reloads, material
//! edits) and otherwise streams per-frame motion: moved or skinned meshes are
//! re-transformed on the CPU into world space (`Renderer::update_mesh`), lights and the
//! sun are rewritten in place. Captures are deterministic and carry numbered marks.
//! Presented frames merge landscape/foliage depth with the scene on the GPU, draw
//! persistent particles, then tone map. Captures can read that exact output path.
#![warn(missing_docs)]

mod composite;
pub mod geometry;
pub mod landscape;
mod readback;
pub mod terrain_mesh;
pub mod two_d;
pub mod vfx;

use anyhow::{Context, Result, ensure};
use ara_render::{
    Renderer,
    capture::{Capture, CaptureRequest, ViewMode},
    scene::{self, CameraSpec, Scene, Vertex},
};
use ara_runtime::{
    components::{Environment, LightKind},
    extract::{Draw, Geometry, RenderFrame},
    model::Model,
};
use geometry::Source;
use glam::Vec3;
use std::{
    collections::BTreeMap,
    ffi::CString,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

/// Per-frame CPU cost of the last `present`, for profiling.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PresentStats {
    /// Whether the scene was rebuilt (structural change).
    pub rebuilt: bool,
    /// Meshes whose vertices were re-uploaded.
    pub updated: usize,
    /// CPU time spent preparing and uploading (excludes GPU rendering).
    pub prepare: Duration,
    /// Time inside `Renderer::render` (records, submits and waits for the GPU).
    pub render: Duration,
}

/// Owns the renderer and the per-draw upload state.
pub struct Presenter {
    renderer: Option<Renderer>,
    effects: vfx::Effects,
    landscapes: landscape::Landscapes,
    terrain: terrain_mesh::TerrainMeshes,
    /// Whether to use terrain mesh fallback instead of the landscape renderer.
    draw_terrain: bool,
    size: [u32; 2],
    extensions: Vec<CString>,
    cache: PathBuf,
    /// (entity, geometry key, material key) of the uploaded scene, in order.
    layout: Vec<(u64, u64, u64)>,
    next_layout: Vec<(u64, u64, u64)>,
    sources: BTreeMap<u64, Arc<Source>>,
    textures: BTreeMap<(String, usize), Arc<scene::Texture>>,
    /// Last uploaded world matrix per draw entity (skinned draws always update).
    uploaded: BTreeMap<u64, [f32; 16]>,
    lights: Vec<scene::Light>,
    sun: ([f32; 3], [f32; 3]),
    scratch: Vec<Vertex>,
    stats: PresentStats,
    composite: Option<composite::Composite>,
    output: Option<ara_gpu::Image>,
}

fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
const FNV: u64 = 0xcbf2_9ce4_8422_2325;

fn geometry_key(geometry: &Geometry) -> u64 {
    match geometry {
        Geometry::Shape { shape, size } => {
            let mut h = fnv(FNV, &[*shape as u8]);
            for v in size {
                h = fnv(h, &v.to_bits().to_le_bytes());
            }
            h
        }
        Geometry::Model { model, primitive } => fnv(
            fnv(FNV, &model.digest.0),
            &(*primitive as u64).to_le_bytes(),
        ),
    }
}

fn material_key(draw: &Draw) -> u64 {
    let mut h = FNV;
    for v in draw
        .color
        .iter()
        .chain([draw.roughness, draw.metallic].iter())
        .chain(draw.emissive.iter())
    {
        h = fnv(h, &v.to_bits().to_le_bytes());
    }
    h
}

impl Presenter {
    /// A presenter for `size` pixels. `extensions` are the window's Vulkan instance
    /// extensions (empty for headless); `cache` holds the pipeline cache.
    pub fn new(size: [u32; 2], extensions: Vec<CString>, cache: impl Into<PathBuf>) -> Self {
        Self {
            renderer: None,
            effects: vfx::Effects::default(),
            landscapes: landscape::Landscapes::default(),
            terrain: terrain_mesh::TerrainMeshes::default(),
            draw_terrain: false,
            size,
            extensions,
            cache: cache.into(),
            layout: Vec::new(),
            next_layout: Vec::new(),
            sources: BTreeMap::new(),
            textures: BTreeMap::new(),
            uploaded: BTreeMap::new(),
            lights: Vec::new(),
            sun: ([0.0; 3], [0.0; 3]),
            scratch: Vec::new(),
            stats: PresentStats::default(),
            composite: None,
            output: None,
        }
    }

    /// The renderer, once the first frame was presented.
    pub fn renderer(&self) -> Option<&Renderer> {
        self.renderer.as_ref()
    }

    /// Final GPU-composited window image, in CopySource state.
    pub fn output(&self) -> Option<ara_gpu::Image> {
        self.output
            .or_else(|| self.renderer.as_ref().map(Renderer::output))
    }

    /// Cost of the last `present`/`prepare`.
    pub fn stats(&self) -> PresentStats {
        self.stats
    }

    /// Resizes the output (history resets).
    pub fn resize(&mut self, size: [u32; 2]) -> Result<()> {
        self.size = size;
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(size)?;
        }
        Ok(())
    }

    /// Uploads `frame`'s content (rebuilding only on structural change).
    pub fn prepare(&mut self, frame: &RenderFrame) -> Result<()> {
        let started = Instant::now();
        ensure!(!frame.draws.is_empty(), "nothing to draw");
        self.next_layout.clear();
        for draw in &frame.draws {
            self.next_layout.push((
                draw.entity,
                geometry_key(&draw.geometry),
                material_key(draw),
            ));
        }
        let tiles = self.terrain_tiles(frame);
        for tile in &tiles {
            self.next_layout.push((tile.entity, tile.key, 0));
        }
        let rebuild = self.renderer.is_none() || self.next_layout != self.layout;
        self.stats = PresentStats {
            rebuilt: rebuild,
            ..Default::default()
        };
        if rebuild {
            let scene = self.scene(frame, &tiles)?;
            match &mut self.renderer {
                Some(renderer) => renderer.replace_scene(scene)?,
                None => {
                    self.renderer = Some(Renderer::new(
                        scene,
                        self.size,
                        &self.extensions,
                        &self.cache,
                    ).context("ARA_GPU_BACKEND: cannot initialize rendering; run `ara doctor` to check the Vulkan loader and GPU driver")?);
                }
            }
            std::mem::swap(&mut self.layout, &mut self.next_layout);
        } else {
            self.update(frame)?;
        }
        self.stats.prepare = started.elapsed();
        Ok(())
    }

    /// Renders the scene, landscape/foliage and particles to one GPU window image.
    pub fn present(&mut self, frame: &RenderFrame) -> Result<()> {
        self.draw_terrain = frame.terrain.is_none();
        self.prepare(frame)?;
        let camera = camera(frame)?;
        let exposure = frame.environment.exposure;
        let started = Instant::now();
        let renderer = self
            .renderer
            .as_mut()
            .context("renderer missing after prepare")?;
        renderer.render(&camera, exposure)?;
        let terrain = self.landscapes.present(renderer, frame, &camera)?;
        let (image, sampled_index, state) = renderer.feature_color();
        let mut color = ara_vfx_gpu::SceneColor {
            image,
            sampled_index,
            state,
        };
        let (image, sampled_index, state) = renderer.feature_depth();
        let mut depth = ara_vfx_gpu::SceneDepth {
            image,
            sampled_index,
            state,
        };
        if terrain.is_some() || !frame.effects.is_empty() {
            if self
                .composite
                .as_ref()
                .is_none_or(|c| c.size != renderer.size())
            {
                if let Some(old) = self.composite.take() {
                    old.retire(renderer.gpu_mut())?;
                }
                let size = renderer.size();
                self.composite = Some(composite::Composite::new(renderer.gpu_mut(), size)?);
            }
            let composite = self.composite.as_mut().context("compositor initialized")?;
            if let Some(terrain) = terrain {
                (color, depth) =
                    composite.merge(renderer.gpu_mut(), color, depth, terrain, exposure)?;
            }
            color = self
                .effects
                .present(renderer, frame, &camera, color, depth)?;
            self.output = Some(composite.finish(renderer.gpu_mut(), color, exposure)?);
        } else {
            self.output = Some(renderer.output());
        }
        self.stats.render = started.elapsed();
        Ok(())
    }

    /// A deterministic capture of `frame` at `size` with up to `marks` numbered marks.
    pub fn capture(
        &mut self,
        frame: &RenderFrame,
        size: [u32; 2],
        mode: ViewMode,
        marks: Option<usize>,
        seed: u64,
    ) -> Result<Capture> {
        // Terrain comes from the landscape composite below.
        self.draw_terrain = false;
        self.prepare(frame)?;
        let request = CaptureRequest {
            camera: camera(frame)?,
            size,
            mode,
            deterministic: true,
            exposure: frame.environment.exposure,
            settle_frames: 4,
            tick: frame.tick,
            seed,
            marks,
        };
        let renderer = self
            .renderer
            .as_mut()
            .context("renderer missing after prepare")?;
        let mut capture = renderer.capture(&request)?;
        let base = frame.terrain.is_some().then(|| capture.image.clone());
        // GPU particles replay over the captured frame before it is resized away.
        self.effects
            .composite(renderer, frame, &request, &mut capture)?;
        // Terrain and foliage by depth, last: VFX deltas carry onto terrain.
        if let Some(base) = base {
            self.landscapes
                .composite(renderer, frame, &request, &base, &mut capture)?;
        }
        // Captures resize to the request; return to the presentation size.
        renderer.resize(self.size)?;
        Ok(capture)
    }

    /// Captures the window presentation path, including landscape/foliage and VFX.
    pub fn capture_presented(
        &mut self,
        frame: &RenderFrame,
        size: [u32; 2],
        marks: Option<usize>,
        seed: u64,
    ) -> Result<Capture> {
        self.draw_terrain = true;
        self.prepare(frame)?;
        let request = CaptureRequest {
            camera: camera(frame)?,
            size,
            mode: ViewMode::Lit,
            deterministic: true,
            exposure: frame.environment.exposure,
            settle_frames: 4,
            tick: frame.tick,
            seed,
            marks,
        };
        let renderer = self
            .renderer
            .as_mut()
            .context("renderer missing after prepare")?;
        let mut capture = renderer.capture(&request)?;
        let original_size = self.size;
        self.size = size;
        self.present(frame)?;
        let output = self.output.context("composited output missing")?;
        readback::replace(
            self.renderer.as_mut().context("renderer missing")?,
            output,
            &mut capture,
        )?;
        self.resize(original_size)?;
        Ok(capture)
    }

    fn source(&mut self, geometry: &Geometry) -> Result<Arc<Source>> {
        let key = geometry_key(geometry);
        if let Some(s) = self.sources.get(&key) {
            return Ok(Arc::clone(s));
        }
        let source = Arc::new(match geometry {
            Geometry::Shape { shape, size } => geometry::shape(*shape, *size),
            Geometry::Model { model, primitive } => {
                let p = model
                    .primitives
                    .get(*primitive)
                    .context("primitive out of range")?;
                Source {
                    vertices: p.vertices.clone(),
                    indices: p.indices.clone(),
                }
            }
        });
        self.sources.insert(key, Arc::clone(&source));
        Ok(source)
    }

    fn texture(&mut self, model: &Model, index: usize) -> Result<Arc<scene::Texture>> {
        let key = (model.digest.to_string(), index);
        if let Some(t) = self.textures.get(&key) {
            return Ok(Arc::clone(t));
        }
        let texture = Arc::new(decode_texture(&model.textures[index].ktx2)?);
        self.textures.insert(key, Arc::clone(&texture));
        Ok(texture)
    }

    /// Meshes of the frame's resident terrain chunks, when this frame draws them.
    fn terrain_tiles(&mut self, frame: &RenderFrame) -> Vec<Arc<terrain_mesh::TileMesh>> {
        let Some(view) = frame.terrain.as_ref().filter(|_| self.draw_terrain) else {
            return Vec::new();
        };
        let mut coords = view.tiles.clone();
        coords.sort();
        coords.dedup();
        coords
            .into_iter()
            .filter_map(|c| self.terrain.tile(&view.data, c))
            .collect()
    }

    fn scene(
        &mut self,
        frame: &RenderFrame,
        tiles: &[Arc<terrain_mesh::TileMesh>],
    ) -> Result<Scene> {
        let mut scene = Scene {
            meshes: Vec::new(),
            materials: Vec::new(),
            textures: vec![
                scene::Texture {
                    size: [1, 1],
                    rgba: vec![255; 4],
                    srgb: false,
                },
                scene::Texture {
                    size: [1, 1],
                    rgba: vec![128, 128, 255, 255],
                    srgb: false,
                },
            ],
            lights: lights(frame),
            sun_direction: glam30::Vec3::from_array(sun(frame).0),
            sun_lux: glam30::Vec3::from_array(sun(frame).1),
            environment: Vec::new(),
            environment_size: [0; 2],
        };
        let (size, sky) = sky(&frame.environment);
        scene.environment_size = size;
        scene.environment = sky;
        let mut texture_slots: BTreeMap<(String, usize), u32> = BTreeMap::new();
        self.uploaded.clear();
        for draw in &frame.draws {
            let source = self.source(&draw.geometry)?;
            let palette = draw.skin.clone().map(|r| &frame.palettes[r]);
            let mut vertices = Vec::new();
            geometry::transform(&source.vertices, draw.world, palette, &mut vertices);
            let mut material = scene::Material {
                base_color: draw.color,
                emissive_roughness: [
                    draw.emissive[0],
                    draw.emissive[1],
                    draw.emissive[2],
                    draw.roughness,
                ],
                surface: [draw.metallic, 1.0, 1.0, 0.5],
                textures: [0, 1, 0, 0],
                flags: [u32::from(draw.color[3] < 1.0) * 2, 0, 0, 0],
            };
            if let Geometry::Model { model, primitive } = &draw.geometry
                && let Some(m) = model.primitives[*primitive]
                    .material
                    .and_then(|m| model.materials.get(m))
            {
                for (out, (a, b)) in material
                    .base_color
                    .iter_mut()
                    .zip(m.base_color.iter().zip(draw.color))
                {
                    *out = a * b;
                }
                material.emissive_roughness =
                    [m.emissive[0], m.emissive[1], m.emissive[2], m.roughness];
                material.surface = [m.metallic, 1.0, 1.0, m.alpha_cutoff];
                material.flags[0] = m.alpha_mode;
                if let Some(t) = m.base_color_texture {
                    let key = (model.digest.to_string(), t);
                    let slot = match texture_slots.get(&key) {
                        Some(&slot) => slot,
                        None => {
                            let texture = self.texture(model, t)?;
                            let slot = scene.textures.len() as u32;
                            scene.textures.push(scene::Texture {
                                size: texture.size,
                                rgba: texture.rgba.clone(),
                                srgb: true,
                            });
                            texture_slots.insert(key, slot);
                            slot
                        }
                    };
                    material.textures[0] = slot;
                }
            }
            scene.materials.push(material);
            scene.meshes.push(scene::Mesh {
                entity: draw.entity,
                name: draw.label.clone(),
                vertices,
                indices: source.indices.clone(),
                material: scene.materials.len() - 1,
            });
            self.uploaded
                .insert(draw.entity, draw.world.to_cols_array());
        }
        for tile in tiles {
            let slot = scene.textures.len() as u32;
            scene.textures.push(scene::Texture {
                size: tile.texture.size,
                rgba: tile.texture.rgba.clone(),
                srgb: true,
            });
            scene.materials.push(scene::Material {
                base_color: [1.0; 4],
                emissive_roughness: [0.0, 0.0, 0.0, 0.95],
                surface: [0.0, 1.0, 1.0, 0.5],
                textures: [slot, 1, 0, 0],
                flags: [0; 4],
            });
            let mut vertices = Vec::new();
            geometry::transform(
                &tile.source.vertices,
                glam::Mat4::IDENTITY,
                None,
                &mut vertices,
            );
            scene.meshes.push(scene::Mesh {
                entity: tile.entity,
                name: tile.label.clone(),
                vertices,
                indices: tile.source.indices.clone(),
                material: scene.materials.len() - 1,
            });
        }
        self.lights = scene.lights.clone();
        self.sun = sun(frame);
        Ok(scene)
    }

    fn update(&mut self, frame: &RenderFrame) -> Result<()> {
        let renderer = self.renderer.as_mut().context("no renderer")?;
        for draw in &frame.draws {
            let matrix = draw.world.to_cols_array();
            let moved = self.uploaded.get(&draw.entity) != Some(&matrix);
            if !moved && draw.skin.is_none() {
                continue;
            }
            let key = geometry_key(&draw.geometry);
            let source = self
                .sources
                .get(&key)
                .context("geometry missing from cache")?;
            let palette = draw.skin.clone().map(|r| &frame.palettes[r]);
            geometry::transform(&source.vertices, draw.world, palette, &mut self.scratch);
            renderer.update_mesh(draw.entity, &self.scratch)?;
            self.uploaded.insert(draw.entity, matrix);
            self.stats.updated += 1;
        }
        let lights = lights(frame);
        if lights.len() != self.lights.len()
            || lights
                .iter()
                .zip(&self.lights)
                .any(|(a, b)| bytes(a) != bytes(b))
        {
            renderer.set_lights(&lights)?;
            self.lights = lights;
        }
        let sun = sun(frame);
        if sun != self.sun {
            renderer.set_sun(
                glam30::Vec3::from_array(sun.0),
                glam30::Vec3::from_array(sun.1),
            )?;
            self.sun = sun;
        }
        Ok(())
    }
}

fn bytes(light: &scene::Light) -> [u32; 16] {
    let mut out = [0; 16];
    for (i, v) in light
        .position_range
        .iter()
        .chain(&light.color_lumens)
        .chain(&light.direction_outer)
        .chain(&light.parameters)
        .enumerate()
    {
        out[i] = v.to_bits();
    }
    out
}

/// The frame's sun, or a dim default so the renderer always has a directional light.
fn sun(frame: &RenderFrame) -> ([f32; 3], [f32; 3]) {
    frame.sun.unwrap_or(([0.3, 1.0, 0.2], [0.0; 3]))
}

fn lights(frame: &RenderFrame) -> Vec<scene::Light> {
    frame
        .lights
        .iter()
        .take(64)
        .map(|l| {
            let [x, y, z] = l.position;
            let [dx, dy, dz] = l.direction;
            let spot = l.kind == LightKind::Spot;
            scene::Light {
                position_range: [x, y, z, l.range],
                color_lumens: [l.color[0], l.color[1], l.color[2], l.lumens],
                direction_outer: [
                    dx,
                    dy,
                    dz,
                    if spot {
                        l.cone[1].to_radians().cos()
                    } else {
                        0.0
                    },
                ],
                parameters: if spot {
                    [1.0, l.cone[0].to_radians().cos(), 0.0, 0.0]
                } else {
                    [0.0; 4]
                },
            }
        })
        .collect()
}

/// The frame's camera, or an overview when the world has none.
pub fn camera(frame: &RenderFrame) -> Result<CameraSpec> {
    Ok(match frame.camera {
        Some(c) => {
            let position = Vec3::from_array(c.position);
            let target = position + Vec3::from_array(c.forward);
            CameraSpec::LookAt {
                position: glam30::Vec3::from_array(position.to_array()),
                target: glam30::Vec3::from_array(target.to_array()),
                vertical_fov: c.fov.to_radians(),
            }
        }
        None => CameraSpec::LookAt {
            position: glam30::Vec3::new(12.0, 10.0, 12.0),
            target: glam30::Vec3::ZERO,
            vertical_fov: 55f32.to_radians(),
        },
    })
}

/// A small equirectangular sky gradient (zenith, horizon, ground) in cd/m².
pub fn sky(env: &Environment) -> ([u32; 2], Vec<[f32; 4]>) {
    const W: u32 = 64;
    const H: u32 = 32;
    let sky = Vec3::from_slice(&env.sky.0[..3]);
    let horizon = Vec3::from_slice(&env.horizon.0[..3]);
    let ground = Vec3::from_slice(&env.ground.0[..3]);
    let mut pixels = Vec::with_capacity((W * H) as usize);
    for y in 0..H {
        let elevation =
            std::f32::consts::FRAC_PI_2 - (y as f32 + 0.5) / H as f32 * std::f32::consts::PI;
        let s = elevation.sin();
        let c = if s >= 0.0 {
            horizon.lerp(sky, s.powf(0.6))
        } else {
            horizon.lerp(ground, (-s * 4.0).min(1.0))
        } * env.intensity;
        for _ in 0..W {
            pixels.push([c.x.max(0.0), c.y.max(0.0), c.z.max(0.0), 1.0]);
        }
    }
    ([W, H], pixels)
}

/// Decodes the finest level of a cooked KTX2 color texture to RGBA8 (BC7 on desktop).
pub fn decode_texture(ktx2: &[u8]) -> Result<scene::Texture> {
    let texture =
        ara_asset_format::Texture::read(ktx2).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let [w, h] = texture.dimensions();
    let level = texture.levels().next().context("texture has no levels")?;
    let mut pixels = vec![0u32; (w * h) as usize];
    match texture.format() {
        ara_asset_format::TextureFormat::Bc7 | ara_asset_format::TextureFormat::Bc7Srgb => {
            texture2ddecoder::decode_bc7(level, w as usize, h as usize, &mut pixels)
                .map_err(|e| anyhow::anyhow!("BC7 decode: {e}"))?;
        }
        format => anyhow::bail!("runtime color textures must be BC7, got {format:?}"),
    }
    let rgba = pixels
        .iter()
        .flat_map(|p| {
            let [b, g, r, a] = p.to_le_bytes();
            [r, g, b, a]
        })
        .collect();
    Ok(scene::Texture {
        size: [w, h],
        rgba,
        srgb: true,
    })
}
