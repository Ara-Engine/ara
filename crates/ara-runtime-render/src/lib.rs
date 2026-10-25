//! Native raster presentation shared by Scene, Play and installed players.
//! [`RasterView`] consumes local shared geometry, compressed textures and moving
//! instances; terrain and foliage enter the same G-buffer and authored look.
//! [`Presenter`] owns the device and completed last-good output. Experimental RT
//! is explicitly enabled through [`RenderOptions`], before device creation.
#![warn(missing_docs)]

extern crate glam34 as glam;

mod composite;
mod emitters;
pub mod fidelity;
pub mod geometry;
mod gi;
mod gi_cpu;
mod gpu_costs;
pub use gpu_costs::GpuFrameCosts;
pub mod landscape;
#[cfg(test)]
mod landscape_tests;
mod material;
mod placeholders;
mod presentation;
pub use placeholders::{ModelPlaceholders, PENDING_MODEL_COLOR};
pub use presentation::{PresentationImage, QualityChoice, QualityPreset, RenderOptions};
mod raster;
pub use raster::{RasterContent, RasterUpdate, RasterView};
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

/// Scene textures (model maps, terrain tiles and the two defaults) the presenter
/// uploads at most: the device's bindless heap holds 128 sampled images in total
/// (the cooked binding ABI), shared with render targets and capture composites.
pub const SCENE_TEXTURE_BUDGET: usize = 32;

/// Per-frame CPU cost of the last `present`, for profiling.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PresentStats {
    /// Native GPU milliseconds, including scene updates and output publication.
    pub gpu_ms: f64,
    /// Native timestamp components and residency counters, on every raster tier
    /// and experimental RT. The legacy diagnostic fallback has incomplete coverage.
    pub gpu: Option<GpuFrameCosts>,
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
    live: Option<presentation::Live>,
    raster: Option<RasterView>,
    published: Option<presentation::Published>,
    choice: QualityChoice,
    plan: Option<ara_render_fidelity::presets::Plan>,
    published_quality: Option<QualityPreset>,
    failure: Option<String>,
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
    /// Whether the over-budget texture note was printed.
    warned_budget: bool,
    /// Background probe GI acquisition for raster content (`set_probe_gi`).
    probe_gi: bool,
    /// Renderer selection (`set_render_options`).
    options: ara_render_fidelity::presets::RenderOptions,
    /// The content's authored environment map (HDRI), replacing the sky gradient.
    environment: Option<fidelity::EnvironmentMap>,
    /// Host look override (`set_look_override`), replacing the frame's authored look.
    look_override: Option<ara_render_fidelity::environment::FrameEnvironment>,
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
    h = fnv(h, &[u8::from(draw.emissive_factor.is_some())]);
    if let Some(factor) = draw.emissive_factor {
        for value in factor {
            h = fnv(h, &value.to_bits().to_le_bytes());
        }
    }
    h = fnv(h, &[u8::from(draw.material.is_some())]);
    if let Some(material) = &draw.material {
        h = fnv(h, &material.digest.0);
    }
    h
}

impl Presenter {
    /// Update loading/failed bounds at an import-status boundary. This changes
    /// only existing native material rows and retains geometry and last-good models.
    pub fn update_model_placeholders(&mut self, is_pending: impl Fn(&str) -> bool) -> Result<()> {
        if let (Some(raster), Some(renderer)) = (&mut self.raster, &mut self.renderer) {
            raster.style_unpublished(renderer, is_pending)?;
        }
        Ok(())
    }
    /// A presenter for `size` pixels. `extensions` are the window's Vulkan instance
    /// extensions (empty for headless); `cache` holds the pipeline cache.
    pub fn new(size: [u32; 2], extensions: Vec<CString>, cache: impl Into<PathBuf>) -> Self {
        Self {
            live: None,
            raster: None,
            published: None,
            choice: QualityChoice::Auto,
            plan: None,
            published_quality: None,
            failure: None,
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
            warned_budget: false,
            probe_gi: true,
            options: ara_render_fidelity::presets::RenderOptions::default(),
            environment: None,
            look_override: None,
        }
    }

    /// The renderer, once the first frame was presented.
    pub fn renderer(&self) -> Option<&Renderer> {
        self.renderer.as_ref()
    }

    /// Final GPU-composited window image, in CopySource state.
    pub fn output(&self) -> Option<ara_gpu::Image> {
        self.published
            .as_ref()
            .map(|image| image.image)
            .or(self.output)
            .or_else(|| self.renderer.as_ref().map(Renderer::output))
    }

    /// Cost of the last `present`/`prepare`.
    pub fn stats(&self) -> PresentStats {
        self.stats
    }

    /// Resizes the output (history resets).
    pub fn resize(&mut self, size: [u32; 2]) -> Result<()> {
        ensure!(!size.contains(&0), "presentation extent must be nonzero");
        self.size = size;
        Ok(())
    }

    /// The look this frame renders with: the host override, or the frame's authored
    /// sky, fog, exposure and display (shared with fidelity).
    fn frame_look(
        &self,
        frame: &RenderFrame,
    ) -> ara_render_fidelity::environment::FrameEnvironment {
        self.look_override.unwrap_or_else(|| {
            ara_render_fidelity::environment::FrameEnvironment::resolve(
                &frame.look,
                frame.environment.exposure,
            )
        })
    }

    /// Uploads `frame`'s content (rebuilding only on structural change). A rebuild
    /// resolves `content` once (its environment map lights and backs the scene)
    /// and returns it for probe GI acquisition.
    fn prepare_baseline(
        &mut self,
        frame: &RenderFrame,
        content: impl FnOnce() -> Result<fidelity::native::Content>,
    ) -> Result<Option<fidelity::native::Content>> {
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
        let mut resolved = None;
        if rebuild {
            let content = content()?;
            self.environment = content
                .environment
                .as_ref()
                .map(|map| fidelity::EnvironmentMap {
                    size: map.size,
                    pixels: map.pixels.clone(),
                });
            resolved = Some(content);
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
        let look = self.frame_look(frame).raster();
        let renderer = self
            .renderer
            .as_mut()
            .context("renderer missing after prepare")?;
        if *renderer.look() != look {
            renderer.set_look(look)?;
        }
        self.stats.prepare = started.elapsed();
        Ok(resolved)
    }

    /// Renders the scene, landscape/foliage and particles to one GPU window image.
    fn present_baseline(&mut self, frame: &RenderFrame) -> Result<()> {
        self.draw_terrain = frame.terrain.is_none();
        let camera = camera(frame)?;
        let exposure = self
            .frame_look(frame)
            .manual_exposure(frame.environment.exposure);
        let started = Instant::now();
        let renderer = self
            .renderer
            .as_mut()
            .context("renderer missing after prepare")?;
        // Landscape and particles composite into the lit HDR before it is graded.
        let native = renderer.is_native();
        let compositing = (!native && frame.terrain.is_some()) || !frame.effects.is_empty();
        renderer.set_deferred_post(compositing);
        renderer.render(&camera, exposure)?;
        let terrain = if native {
            None
        } else {
            self.landscapes.present(renderer, frame, &camera)?
        };
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
        if compositing {
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
            self.output = Some(renderer.finish(color.image, color.sampled_index, color.state)?);
        } else {
            self.output = Some(renderer.output());
        }
        if native {
            let textures = renderer
                .native_texture_stats()
                .context("native texture statistics")?;
            let costs = GpuFrameCosts {
                scene_update_ms: renderer.native_update_gpu_ms().unwrap_or(0.),
                texture_upload_ms: textures.upload_gpu_ms,
                texture_uploaded_bytes: textures.uploaded_bytes,
                texture_pending: textures.pending_textures,
                texture_resident_bytes: textures.resident_bytes,
                effects_ms: if frame.effects.is_empty() {
                    0.
                } else {
                    self.effects.gpu_ms()
                },
                raster_ms: renderer
                    .timings()
                    .iter()
                    .map(|pass| pass.milliseconds)
                    .sum(),
                ..Default::default()
            };
            self.stats.gpu_ms = costs.accounted_ms();
            self.stats.gpu = Some(costs);
        }
        self.stats.render = started.elapsed();
        Ok(())
    }

    /// A deterministic capture of `frame` at `size` with up to `marks` numbered marks.
    fn capture_baseline(
        &mut self,
        frame: &RenderFrame,
        size: [u32; 2],
        mode: ViewMode,
        marks: Option<usize>,
        seed: u64,
    ) -> Result<Capture> {
        // Terrain comes from the landscape composite below.
        self.draw_terrain = false;
        if self.raster.is_none() {
            self.prepare_baseline(frame, || Ok(fidelity::native::Content::default()))?;
        }
        self.wait_probe_gi()?;
        let request = CaptureRequest {
            camera: camera(frame)?,
            size,
            mode,
            // Renderer::capture resets the jitter sequence and history itself;
            // live TAA can remain enabled in a reproducible Low capture.
            deterministic: false,
            exposure: self
                .frame_look(frame)
                .manual_exposure(frame.environment.exposure),
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
        let base =
            (!renderer.is_native() && frame.terrain.is_some()).then(|| capture.image.clone());
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
    fn capture_presented_baseline(
        &mut self,
        frame: &RenderFrame,
        size: [u32; 2],
        marks: Option<usize>,
        seed: u64,
    ) -> Result<Capture> {
        self.draw_terrain = true;
        self.prepare(frame)?;
        self.wait_probe_gi()?;
        let request = CaptureRequest {
            camera: camera(frame)?,
            size,
            mode: ViewMode::Lit,
            deterministic: true,
            exposure: self
                .frame_look(frame)
                .manual_exposure(frame.environment.exposure),
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

    fn texture(
        &mut self,
        source: material::Source<'_>,
        index: usize,
    ) -> Result<Arc<scene::Texture>> {
        let key = source.key(index);
        if let Some(t) = self.textures.get(&key) {
            return Ok(Arc::clone(t));
        }
        let mut texture = decode_texture(&source.texture(index).ktx2)?;
        if source.raw() {
            texture.srgb = false;
        }
        let texture = Arc::new(texture);
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
        let (size, sky) = match &self.environment {
            Some(map) => (map.size, map.pixels.clone()),
            None => sky(&frame.environment),
        };
        scene.environment_size = size;
        scene.environment = sky;
        // Model textures share the device's 128-entry sampled-image heap with the
        // renderer's own targets: colour maps of every draw come first, then normal,
        // metal/roughness and emissive maps while the budget lasts. Maps past it fold
        // into their material's factors (mean texel) instead of failing the upload.
        let budget = SCENE_TEXTURE_BUDGET.saturating_sub(2 + tiles.len());
        let mut wanted: Vec<Vec<(material::Source<'_>, usize)>> = vec![Vec::new(); 4];
        let mut seen = std::collections::BTreeSet::new();
        for draw in &frame.draws {
            if let Some(source) = material::Source::of(draw) {
                let m = source.material();
                for (slot, texture) in [
                    m.base_color_texture,
                    m.normal_texture,
                    m.metallic_roughness_texture,
                    m.emissive_texture,
                ]
                .into_iter()
                .enumerate()
                {
                    if let Some(t) = texture
                        && seen.insert((slot, source.key(t)))
                    {
                        wanted[slot].push((source, t));
                    }
                }
            }
        }
        let mut accepted = std::collections::BTreeSet::new();
        for (source, t) in wanted.iter().flatten() {
            let key = source.key(*t);
            if accepted.len() < budget || accepted.contains(&key) {
                accepted.insert(key);
            }
        }
        let folded = seen.len().saturating_sub(accepted.len());
        if folded > 0 && !self.warned_budget {
            eprintln!(
                "ara-runtime-render: {folded} model texture maps exceed the {SCENE_TEXTURE_BUDGET}-slot scene budget; folded into material factors"
            );
            self.warned_budget = true;
        }
        let mut texture_slots: BTreeMap<(String, usize), u32> = BTreeMap::new();
        self.uploaded.clear();
        for draw in &frame.draws {
            let source = self.source(&draw.geometry)?;
            let palette = draw.skin.clone().map(|r| &frame.palettes[r]);
            let mut vertices = Vec::new();
            geometry::transform(&source.vertices, draw.world, palette, &mut vertices);
            // Basic has one UV stream; native overrides use the base-color affine
            // coordinates. The fidelity consumer retains each slot's independent UVs.
            if let Some(native) = draw.material.as_ref() {
                let uv = native.material.base_color_uv;
                for vertex in &mut vertices {
                    vertex.uv = uv.apply(vertex.uv);
                }
            }
            // A model without a material has zero source emission; shapes use radiance.
            let default_emissive = if matches!(&draw.geometry, Geometry::Model { .. }) {
                let factor = draw.emissive_factor.unwrap_or([0.; 3]);
                std::array::from_fn(|i| factor[i] * draw.emissive[i])
            } else {
                draw.emissive
            };
            let mut material = scene::Material {
                base_color: draw.color,
                emissive_roughness: [
                    default_emissive[0],
                    default_emissive[1],
                    default_emissive[2],
                    draw.roughness,
                ],
                surface: [draw.metallic, 1.0, 1.0, 0.5],
                textures: [0, 1, 0, 0],
                flags: [u32::from(draw.color[3] < 1.0) * 2, 0, 0, 0],
            };
            if let Some(source) = material::Source::of(draw) {
                let m = source.material();
                for (out, (a, b)) in material
                    .base_color
                    .iter_mut()
                    .zip(m.base_color.iter().zip(draw.color))
                {
                    *out = a * b;
                }
                let emissive = draw.emissive_factor.unwrap_or(m.emissive);
                material.emissive_roughness = [
                    emissive[0] * draw.emissive[0],
                    emissive[1] * draw.emissive[1],
                    emissive[2] * draw.emissive[2],
                    m.roughness,
                ];
                material.surface = [m.metallic, m.normal_scale, 1.0, m.alpha_cutoff];
                material.flags[0] = m.alpha_mode;
                for (slot, texture) in [
                    m.base_color_texture,
                    m.normal_texture,
                    m.metallic_roughness_texture,
                    m.emissive_texture,
                ]
                .into_iter()
                .enumerate()
                {
                    let Some(t) = texture else { continue };
                    let key = source.key(t);
                    if !accepted.contains(&key) {
                        let mean = if source.raw() {
                            let texture = self.texture(source, t)?;
                            let mut mean = [0.; 4];
                            for pixel in texture.rgba.as_chunks::<4>().0 {
                                for i in 0..4 {
                                    mean[i] += f32::from(pixel[i]) / 255.;
                                }
                            }
                            for channel in &mut mean {
                                *channel /= (texture.size[0] * texture.size[1]) as f32;
                            }
                            if source.decode(slot) {
                                for channel in &mut mean[..3] {
                                    *channel = if *channel <= 0.04045 {
                                        *channel / 12.92
                                    } else {
                                        ((*channel + 0.055) / 1.055).powf(2.4)
                                    };
                                }
                            }
                            mean
                        } else {
                            mean_texel(&source.texture(t).ktx2)?
                        };
                        fold(&mut material, slot, mean);
                        continue;
                    }
                    let index = match texture_slots.get(&key) {
                        Some(&index) => index,
                        None => {
                            let texture = self.texture(source, t)?;
                            let index = scene.textures.len() as u32;
                            scene.textures.push(scene::Texture {
                                size: texture.size,
                                rgba: texture.rgba.clone(),
                                srgb: texture.srgb,
                            });
                            texture_slots.insert(key, index);
                            index
                        }
                    };
                    material.textures[slot] = index;
                    if source.decode(slot) {
                        material.flags[1] |= if slot == 0 {
                            scene::BASE_COLOR_DECODE_SRGB
                        } else {
                            scene::EMISSIVE_DECODE_SRGB
                        };
                    }
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

/// Folds a texture map that did not fit the budget into its material's factors.
fn fold(material: &mut scene::Material, slot: usize, mean: [f32; 4]) {
    match slot {
        0 => {
            for (c, m) in material.base_color.iter_mut().zip(mean).take(3) {
                *c = (*c * m).clamp(0.0, 1.0);
            }
        }
        2 => {
            material.emissive_roughness[3] *= mean[1];
            material.surface[0] *= mean[2];
        }
        3 => {
            for (c, m) in material.emissive_roughness.iter_mut().zip(mean).take(3) {
                *c *= m;
            }
        }
        // Normal maps fold to the flat default.
        _ => {}
    }
}

/// The mean texel of a cooked texture from its coarsest block-sized level (linear
/// colour for sRGB maps, raw channels otherwise).
pub fn mean_texel(ktx2: &[u8]) -> Result<[f32; 4]> {
    let texture =
        ara_asset_format::Texture::read(ktx2).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let [mut w, mut h] = texture.dimensions();
    let mut chosen = None;
    for level in texture.levels() {
        if w < 4 || h < 4 {
            break;
        }
        chosen = Some((level, w, h));
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    let Some((level, w, h)) = chosen else {
        return Ok([1.0; 4]);
    };
    let mut pixels = vec![0u32; (w * h) as usize];
    let format = texture.format();
    match format {
        ara_asset_format::TextureFormat::Bc7 | ara_asset_format::TextureFormat::Bc7Srgb => {
            texture2ddecoder::decode_bc7(level, w as usize, h as usize, &mut pixels)
                .map_err(|e| anyhow::anyhow!("BC7 decode: {e}"))?;
        }
        _ => return Ok([1.0; 4]),
    }
    let srgb = format == ara_asset_format::TextureFormat::Bc7Srgb;
    let mut sum = [0.0f64; 4];
    for p in &pixels {
        let [b, g, r, a] = p.to_le_bytes();
        for (i, v) in [r, g, b, a].into_iter().enumerate() {
            let v = f64::from(v) / 255.0;
            sum[i] += if srgb && i < 3 {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            } else {
                v
            };
        }
    }
    let n = pixels.len().max(1) as f64;
    Ok(sum.map(|s| (s / n) as f32))
}

/// Decodes the finest level of a cooked KTX2 texture to RGBA8: BC7 colour/linear data,
/// or BC5 normals with Z reconstructed (`srgb` reports the colour space).
pub fn decode_texture(ktx2: &[u8]) -> Result<scene::Texture> {
    let texture =
        ara_asset_format::Texture::read(ktx2).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let [w, h] = texture.dimensions();
    let level = texture.levels().next().context("texture has no levels")?;
    let mut pixels = vec![0u32; (w * h) as usize];
    let format = texture.format();
    match format {
        ara_asset_format::TextureFormat::Bc7 | ara_asset_format::TextureFormat::Bc7Srgb => {
            texture2ddecoder::decode_bc7(level, w as usize, h as usize, &mut pixels)
                .map_err(|e| anyhow::anyhow!("BC7 decode: {e}"))?;
        }
        ara_asset_format::TextureFormat::Bc5 => {
            texture2ddecoder::decode_bc5(level, w as usize, h as usize, &mut pixels)
                .map_err(|e| anyhow::anyhow!("BC5 decode: {e}"))?;
        }
        format => anyhow::bail!("runtime textures must be BC7 or BC5 normals, got {format:?}"),
    }
    let normal = format == ara_asset_format::TextureFormat::Bc5;
    let rgba = pixels
        .iter()
        .flat_map(|p| {
            let [b, g, r, a] = p.to_le_bytes();
            if normal {
                // BC5 stores tangent-space X/Y; rebuild Z for the renderer's RGB normal.
                let x = f32::from(r) / 127.5 - 1.0;
                let y = f32::from(g) / 127.5 - 1.0;
                let z = (1.0 - x * x - y * y).max(0.0).sqrt();
                [r, g, ((z * 0.5 + 0.5) * 255.0).round() as u8, 255]
            } else {
                [r, g, b, a]
            }
        })
        .collect();
    Ok(scene::Texture {
        size: [w, h],
        rgba,
        srgb: format == ara_asset_format::TextureFormat::Bc7Srgb,
    })
}
