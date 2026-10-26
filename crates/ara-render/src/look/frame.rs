//! The look constants record (`Look` in shaders/look.slangh).
use super::Look;
use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// Shader flag: the physical sky replaces the environment map.
pub(crate) const PHYSICAL_SKY: u32 = 1;
/// Shader flag: height fog is enabled.
pub(crate) const FOG: u32 = 2;
/// Shader flag: history restarts; probe GI relights without hysteresis.
pub(crate) const RESET: u32 = 4;
/// Shader flag: screen-space ambient occlusion is available.
pub(crate) const AMBIENT_OCCLUSION: u32 = 8;
/// Shader flag: screen-space reflections are available.
pub(crate) const REFLECTIONS: u32 = 16;
/// Shader flag: automatic exposure takes its meter this frame instead of adapting.
pub(crate) const SNAP: u32 = 32;
/// Shader flag: probe GI relight and the fog sample local-light shadows.
pub(crate) const LOCAL_INDIRECT: u32 = 64;
/// Exposure adaptation step; the renderer has no wall clock.
const FRAME_SECONDS: f32 = 1. / 60.;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct LookFrame {
    pub sun_lux: [f32; 4],
    pub atmosphere: [f32; 4],
    pub fog: [f32; 4],
    pub look: [f32; 4],
    pub grading: [f32; 4],
    pub exposure: [f32; 4],
    pub metering: [f32; 4],
    pub bloom: [f32; 4],
    pub flags: [u32; 4],
    pub sampled: [[u32; 4]; 2],
    pub storage: [[u32; 4]; 2],
    pub state: [u32; 4],
    /// x irradiance hysteresis.
    pub gi_params: [f32; 4],
    /// x lattices in the probe pool, w probes (zero: no probe GI).
    pub gi_counts: [u32; 4],
    pub gi_buffers: [u32; 4],
    pub gi_update: [u32; 4],
    pub gi_images: [u32; 4],
    pub ao: [f32; 4],
    pub ao_images: [u32; 4],
    pub gi_gather: [u32; 4],
    pub ssr: [f32; 4],
    pub ssr_images: [u32; 4],
    pub weather: [f32; 4],
    /// Light view-projection of the far shadow map.
    pub far_shadow: [[f32; 4]; 4],
    /// x far map (sampled), y texels per side, z drawn (0: cascades fade to lit).
    pub far_info: [u32; 4],
    /// Sampled cascade maps, in cascade order.
    pub cascade_maps: [u32; 4],
    /// x: terrain detail distance in metres (`Quality::terrain_detail`).
    pub terrain: [f32; 4],
    /// Local-light shadows (`look::local`): x shading table (storage), y atlas
    /// (sampled; zero: none), z/w atlas texels.
    pub local_shadows: [u32; 4],
    /// Heightfield horizon (`look::horizon`): world x, z origin and x, z size.
    pub horizon_rect: [f32; 4],
    /// Height = x * depth + y of the capture; z top-surface tolerance (m).
    pub horizon_height: [f32; 4],
    /// x capture (sampled), y sun visibility, z sky visibility (sampled; zero:
    /// none), w capture texels per side.
    pub horizon_images: [u32; 4],
    /// x sun visibility, y sky visibility (storage), z sky texels per side.
    pub horizon_storage: [u32; 4],
    /// Virtual shadow maps (`look::vsm`): x page table, y atlas (sampled; zero:
    /// none), z page light views (storage), w atlas pages per row.
    pub vsm: [u32; 4],
    /// x this frame's page-request bits (storage; zero: no marks), z atlas page rows.
    pub vsm_request: [u32; 4],
    /// The sun's light rotation.
    pub vsm_space: [[f32; 4]; 4],
    /// depth = x * light z + y; z level bias; w level zero's radius (m).
    pub vsm_depth: [f32; 4],
    /// x projected page visibility (sampled; zero: none this frame), y (storage).
    pub vsm_mask: [u32; 4],
    /// Probe lattices, finest first (look/gi.rs `ProbeLayout`): xyz first probe
    /// centre, w edge falloff in cells.
    pub gi_lattice_origin: [[f32; 4]; super::gi::MAX_LATTICES],
    /// xyz probe separation, w surface bias (m).
    pub gi_lattice_spacing: [[f32; 4]; super::gi::MAX_LATTICES],
    /// xyz dimensions, w first global probe id.
    pub gi_lattice_counts: [[u32; 4]; super::gi::MAX_LATTICES],
}

/// Per-frame inputs that are not part of the authored look.
pub(crate) struct FrameInputs {
    /// Top-of-atmosphere sun illuminance in lux.
    pub sun_lux: Vec3,
    /// Manual linear exposure multiplier; used unless the look meters automatically.
    pub exposure: f32,
    /// Drop GI hysteresis (first frame, history reset or a capture's settle
    /// frames, `LookGpu::settle`).
    pub reset: bool,
    /// Automatic exposure takes its meter instead of adapting (a reset without an
    /// exposure seed, or new probe GI arriving).
    pub snap: bool,
    pub frame_number: u32,
    pub sampled: [[u32; 4]; 2],
    pub storage: [[u32; 4]; 2],
    pub state: u32,
}

impl LookFrame {
    pub(crate) fn new(look: &Look, inputs: &FrameInputs) -> Self {
        let sky = look.sky.unwrap_or_default();
        let fog = look.fog;
        let g = &look.grading;
        let [r, gr, b] = g.white_balance_gain;
        let automatic = look.auto_exposure;
        let metering = look.auto_exposure.unwrap_or_default();
        let flags = if look.sky.is_some() { PHYSICAL_SKY } else { 0 }
            | if fog.is_some() { FOG } else { 0 }
            | if inputs.reset { RESET } else { 0 }
            | if inputs.snap { SNAP } else { 0 };
        Self {
            sun_lux: inputs.sun_lux.extend(sky.sun_radius).to_array(),
            atmosphere: [
                sky.sea_level,
                fog.map_or(0., |f| f.extinction),
                fog.map_or(1., |f| f.height),
                fog.map_or(0., |f| f.albedo),
            ],
            fog: fog.map_or([1., 1., 1., 0.], |f| {
                let [r, g, b] = f.tint;
                [r, g, b, f.anisotropy]
            }),
            look: [
                g.contrast,
                g.vignette,
                fog.map_or(0., |f| f.base),
                sky.aerosols,
            ],
            grading: [r, gr, b, g.saturation],
            exposure: [
                if automatic.is_some() {
                    0.
                } else {
                    inputs.exposure
                },
                2f32.powf(metering.compensation),
                metering.adaptation,
                FRAME_SECONDS,
            ],
            metering: [
                metering.minimum_ev100,
                metering.maximum_ev100,
                metering.low_percentile,
                metering.high_percentile,
            ],
            bloom: [look.bloom, 0., 0., 0.],
            flags: [look.curve as u32, flags, inputs.frame_number, 0],
            sampled: inputs.sampled,
            storage: inputs.storage,
            state: [inputs.state, 0, 0, 0],
            gi_params: [0.; 4],
            gi_counts: [0; 4],
            gi_buffers: [0; 4],
            gi_update: [0; 4],
            gi_images: [0; 4],
            ao: [0.; 4],
            ao_images: [0; 4],
            gi_gather: [0; 4],
            ssr: [0.; 4],
            ssr_images: [0; 4],
            weather: [sky.cloud_coverage, sky.cloud_height, 0., 0.],
            far_shadow: [[0.; 4]; 4],
            far_info: [0; 4],
            cascade_maps: [0; 4],
            terrain: [0.; 4],
            local_shadows: [0; 4],
            horizon_rect: [0.; 4],
            horizon_height: [0.; 4],
            horizon_images: [0; 4],
            horizon_storage: [0; 4],
            vsm: [0; 4],
            vsm_request: [0; 4],
            vsm_space: [[0.; 4]; 4],
            vsm_depth: [0.; 4],
            vsm_mask: [0; 4],
            gi_lattice_origin: [[0., 0., 0., 1.]; super::gi::MAX_LATTICES],
            gi_lattice_spacing: [[1., 1., 1., 0.]; super::gi::MAX_LATTICES],
            gi_lattice_counts: [[2, 2, 2, 0]; super::gi::MAX_LATTICES],
        }
    }
}
