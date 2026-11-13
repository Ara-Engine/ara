//! What the Renderlab players and the editor's Game view draw with: [`Presenter`] owns a
//! [`PresentationDevice`], the external renderer installed on it ([`ExternalRenderer`], in
//! practice Renderlab) and the completed last-good output ([`PresentationImage`]). The crate
//! also resolves a frame's content (`fidelity`: models, materials, terrain, foliage and
//! decals), its look ([`environment_at`]) and its camera.
#![warn(missing_docs)]

extern crate glam34 as glam;

mod debug;
pub use debug::{ReferenceTrace, ReflectionView, RenderDebug};
mod device;
pub use device::PresentationDevice;
mod external;
pub use external::{EXTERNAL_PASSES, ExternalFrame, ExternalRenderer, ExternalView};
pub mod fidelity;
pub mod geometry;
mod gpu_costs;
pub mod graphics;
pub use gpu_costs::GpuFrameCosts;
mod lights;
mod material;
mod placeholders;
mod presentation;
pub use placeholders::{ModelPlaceholders, PENDING_MODEL_COLOR};
pub use presentation::{PresentationImage, QualityChoice, QualityPreset, RenderOptions};
mod readback;
mod readiness;
pub use readiness::PresentationReadiness;
pub mod terrain_mesh;
pub mod two_d;

use anyhow::{Context, Result, ensure};
use ara_frame::{
    capture::{Capture, CaptureRequest, ViewMode},
    scene::{self, CameraSpec},
};
use ara_runtime::{
    components::Environment,
    extract::{Geometry, RenderFrame},
};
use glam::Vec3;
use std::{ffi::CString, path::PathBuf, time::Duration};

/// Per-frame cost of the last `present`, for profiling.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PresentStats {
    /// GPU milliseconds the renderer reported, including output publication.
    pub gpu_ms: f64,
    /// Whole presenter device span, from its first submission to completed output
    /// publication. Includes inter-submission gaps; None means no comparable clock.
    pub gpu_span_ms: Option<f64>,
    /// Gaps between the frame's submission intervals, merging overlapping queues.
    /// This excludes waits inside a submission; unavailable clocks stay None.
    pub gpu_submission_gap_ms: Option<f64>,
    /// GPU components of the frame: the renderer's span and the output publication.
    pub gpu: Option<GpuFrameCosts>,
    /// Whether the scene was rebuilt (structural change).
    pub rebuilt: bool,
    /// CPU time spent opening the device and resolving content (excludes rendering).
    pub prepare: Duration,
    /// Time inside the external renderer's `render`, including the wait for its image.
    pub render: Duration,
    /// This present has no new completed image; hosts skip the UI, the blit or the
    /// publish for it and keep pumping events.
    pub skipped: bool,
    /// Wall milliseconds the external renderer spent moving its image onto the
    /// presenter's device.
    pub external_handoff_ms: Option<f64>,
}

/// Owns the presentation device, the external renderer drawing on it and the completed
/// output.
pub struct Presenter {
    graphics: Option<ara_graphics_settings::Settings>,
    choice: QualityChoice,
    /// The quality preset the device's capabilities resolve `choice` to.
    preset: Option<QualityPreset>,
    published: Option<PresentationImage>,
    failure: Option<String>,
    /// The device external renderers draw on, opened on the first present.
    device: Option<PresentationDevice>,
    initial_device: Option<ara_gpu::GpuDevice>,
    size: [u32; 2],
    extensions: Vec<CString>,
    cache: PathBuf,
    stats: PresentStats,
    /// Renderer selection (`set_render_options`).
    options: ara_render_fidelity::presets::RenderOptions,
    /// The content's authored environment map (HDRI), replacing the sky gradient.
    environment: Option<fidelity::EnvironmentMap>,
    /// Host look override (`set_look_override`), replacing the frame's authored look.
    look_override: Option<ara_render_fidelity::environment::FrameEnvironment>,
    /// Diagnosis switches (`set_debug`): the exposure lock, the debug view and what an
    /// external renderer must report it cannot honour.
    debug: RenderDebug,
    /// The renderer drawing every frame (`set_external_renderer`).
    external: Option<Box<dyn ExternalRenderer>>,
    /// Draw count the external path last resolved content for (its environment map).
    external_draws: Option<usize>,
    /// Present with the external renderer one frame behind, so the CPU records frame N while the
    /// GPU draws frame N-1 (`Presenter::set_external_pipelined`).
    external_pipelined: bool,
    /// Foliage geometry by rule asset from the content the external path last resolved.
    external_foliage: std::collections::BTreeMap<String, Vec<ara_runtime::extract::Draw>>,
    /// Terrain layer materials by reference from that same content.
    external_terrain_materials:
        std::collections::BTreeMap<String, std::sync::Arc<ara_runtime::material::MaterialAsset>>,
    /// The external renderer's own pass timings for its last frame (`ExternalFrame::passes`).
    external_passes: [(&'static str, f64); EXTERNAL_PASSES],
    /// The last frame's pass timings (`pass_timings`).
    timings: Vec<ara_gpu::commands::PassTiming>,
    /// Private recorder identity for the next frame, independent of simulation.
    profile_next: Option<u64>,
    /// Timings paired with the CPU frame that submitted them (`take_profile_timings`).
    profile_completed: std::collections::VecDeque<(u64, Vec<ara_gpu::commands::PassTiming>)>,
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

impl Presenter {
    /// A presenter for `size` pixels. `extensions` are the window's Vulkan instance
    /// extensions (empty for headless); `cache` holds the pipeline cache.
    pub fn new(size: [u32; 2], extensions: Vec<CString>, cache: impl Into<PathBuf>) -> Self {
        Self {
            published: None,
            choice: QualityChoice::Auto,
            preset: None,
            graphics: None,
            failure: None,
            device: None,
            initial_device: None,
            size,
            extensions,
            cache: cache.into(),
            stats: PresentStats::default(),
            options: ara_render_fidelity::presets::RenderOptions::default(),
            environment: None,
            look_override: None,
            debug: RenderDebug::default(),
            external: None,
            external_draws: None,
            external_pipelined: false,
            external_foliage: Default::default(),
            external_terrain_materials: Default::default(),
            external_passes: [("", 0.); EXTERNAL_PASSES],
            timings: Vec::new(),
            profile_next: None,
            profile_completed: std::collections::VecDeque::new(),
        }
    }

    /// Adopt an unused GPU owner before the first present.
    pub fn with_device(size: [u32; 2], cache: impl Into<PathBuf>, gpu: ara_gpu::GpuDevice) -> Self {
        let mut presenter = Self::new(size, Vec::new(), cache);
        presenter.initial_device = Some(gpu);
        presenter
    }

    /// The device external renderers draw on, once the first frame was presented.
    pub fn gpu(&self) -> Option<&ara_gpu::GpuDevice> {
        self.device.as_ref().map(PresentationDevice::gpu)
    }

    /// The device, mutably. Callers that submit or read back through it first finish
    /// the frame they present (presents are synchronous, so nothing is in flight).
    pub fn gpu_mut(&mut self) -> Option<&mut ara_gpu::GpuDevice> {
        self.device.as_mut().map(PresentationDevice::gpu_mut)
    }

    /// The linear exposure multiplier `frame` renders with when it is manual or locked
    /// (`--ev100`, the exposure debug lock); `None` under automatic metering, which the
    /// external renderer owns.
    pub fn applied_exposure(&self, frame: &RenderFrame) -> Option<f32> {
        let look = self.frame_look(frame);
        look.ev100
            .map(|_| look.manual_exposure(frame.environment.exposure))
    }

    /// Final GPU-composited window image, in CopySource state.
    pub fn output(&self) -> Option<ara_gpu::Image> {
        self.published.as_ref().map(PresentationImage::output)
    }

    /// Cost of the last `present`.
    pub fn stats(&self) -> PresentStats {
        self.stats
    }

    /// Cumulative host time in this device's explicit GPU/presentation waits.
    /// Snapshot around a whole windowed frame, including swapchain presentation.
    /// No counter exists until the device has been created.
    pub fn host_wait_stats(&self) -> Option<ara_gpu::vulkan::HostWaitStats> {
        self.gpu().map(ara_gpu::GpuDevice::host_wait_stats)
    }

    /// Resizes the output.
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
        let mut look = self
            .look_override
            .unwrap_or_else(|| environment_locked(frame, self.debug.exposure));
        if let Some(settings) = &self.graphics {
            if !settings.bloom {
                look.bloom = 0.;
            }
            if !settings.motion_blur {
                look.lens.motion_blur = 0.;
            }
            if !settings.depth_of_field {
                look.lens.depth_of_field = None;
            }
        }
        look
    }
}

/// Where [`camera`] looks from when the world has no camera.
const OVERVIEW: [f32; 3] = [12.0, 10.0, 12.0];

/// The frame's authored look as seen from [`camera`] ([`environment_at`] its eye).
pub fn environment(frame: &RenderFrame) -> ara_render_fidelity::environment::FrameEnvironment {
    let eye = frame.camera.map_or(OVERVIEW, |c| c.position);
    environment_at(frame, glam30::Vec3::from_array(eye))
}

/// [`environment`] under a diagnostic exposure lock (`RenderDebug::exposure`, EV100):
/// manual exposure without compensation or its curve, over every volume.
pub fn environment_locked(
    frame: &RenderFrame,
    lock: Option<f32>,
) -> ara_render_fidelity::environment::FrameEnvironment {
    let mut look = environment(frame);
    if let Some(ev100) = lock {
        look.ev100 = Some(ev100.clamp(-24., 32.));
        look.compensation = 0.;
        look.compensation_curve = Default::default();
    }
    look
}

/// The frame's authored look as seen from `eye` (an editor's Scene view camera):
/// post-process volumes blend by that position; the sky light comes from the
/// frame's `Environment`, the atmosphere lights from its directional lights. Every
/// view resolves its look here, so none misses a term.
pub fn environment_at(
    frame: &RenderFrame,
    eye: glam30::Vec3,
) -> ara_render_fidelity::environment::FrameEnvironment {
    use ara_frame::look::atmosphere::{AtmosphereLight, LightAtmosphere};
    let settings = |a: ara_runtime::extract::AtmosphereLightView| LightAtmosphere {
        disc_scale: a.disc_scale,
        per_pixel_transmittance: a.per_pixel_transmittance,
    };
    let mut environment = ara_render_fidelity::environment::FrameEnvironment {
        sky_light: frame.environment.sky_light,
        sun_scattering: frame.sun_volumetric.unwrap_or(1.),
        ..ara_render_fidelity::environment::FrameEnvironment::resolve(
            &frame.look,
            eye,
            frame.environment.exposure,
        )
    };
    environment.set_atmosphere_lights(
        frame.sun.and(frame.sun_atmosphere).map(settings),
        frame.sun_atmosphere_index == 1,
        frame.other_atmosphere_light.and_then(|o| {
            Some(AtmosphereLight {
                direction: glam30::Vec3::from_array(o.direction),
                lux: glam30::Vec3::from_array(o.lux),
                radius: o.radius.clamp(0., 0.2),
                settings: settings(o.atmosphere?),
            })
        }),
    );
    environment
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
            position: glam30::Vec3::from_array(OVERVIEW),
            target: glam30::Vec3::ZERO,
            vertical_fov: 55f32.to_radians(),
        },
    })
}

/// A small equirectangular sky gradient in cd/m², graded as [`Environment`] documents
/// (Unreal's sky-dome form: m = 1 − sin(elevation) blends zenith into horizon and
/// shapes the additive glow band). 128 rows keep a narrow glow band resolved.
pub fn sky(env: &Environment) -> ([u32; 2], Vec<[f32; 4]>) {
    const W: u32 = 64;
    const H: u32 = 128;
    let sky = Vec3::from_slice(&env.sky.0[..3]);
    let horizon = Vec3::from_slice(&env.horizon.0[..3]);
    let glow = Vec3::from_slice(&env.glow.0[..3]);
    let ground = Vec3::from_slice(&env.ground.0[..3]);
    let at_horizon = horizon + glow;
    let mut pixels = Vec::with_capacity((W * H) as usize);
    for y in 0..H {
        let elevation =
            std::f32::consts::FRAC_PI_2 - (y as f32 + 0.5) / H as f32 * std::f32::consts::PI;
        let s = elevation.sin();
        let c = if s >= 0.0 {
            let m = 1.0 - s;
            sky.lerp(horizon, m.powf(env.sky_falloff)) + glow * m.powf(env.glow_falloff)
        } else {
            at_horizon.lerp(ground, (-s * 4.0).min(1.0))
        } * env.intensity;
        for _ in 0..W {
            pixels.push([c.x.max(0.0), c.y.max(0.0), c.z.max(0.0), 1.0]);
        }
    }
    ([W, H], pixels)
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
