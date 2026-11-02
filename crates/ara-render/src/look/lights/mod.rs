//! Finite-range many-light shading, V2 (council-v3 architecture D6, section 3.2, gate
//! G1): a light grid over authored ranges, per-pixel visibility through `trace()`, and
//! omission-only overload within a 1% energy bound. `look::local` stays the default
//! and the A/B control. Owned by the B1 build track.
//!
//! The kernel (`shaders/lights-v2.slang`, [`gpu::LightsGpu`]) runs after the G-buffer:
//! - **build**: each light's sphere at its authored range (never shortened), source
//!   radius and intensity; **bin**: each 64x64 coarse tile's lights, in light order;
//! - **prep**: each 16x16 tile's receiver box (surface points and normal-offset ray
//!   origins), normal cone and BRDF bound;
//! - **select**: each tile's lights (range, facing, one-sided emitters), each pair
//!   classified against trace()'s instances with a conservative capsule over the
//!   tile box and the light's sample sphere, walked through the TLAS and then the
//!   BLAS down to triangle planes: clear pairs shade without rays, partial pairs name
//!   at most two instances their segments may meet (more: the whole TLAS);
//! - **shade**: `localLight` (the shading `look::local` uses: points, spots, spheres,
//!   tubes, rects, discs) times per-source-sample visibility for partial pairs.
//!
//! Visibility is exact geometry: one first-hit segment per fixed source sample, so
//! every occluder combines per sample before the samples average (two blockers over
//! the same half of a source give 0.5). There is no cached per-light depth atlas
//! (killed, R5-1 A), no stochastic pattern and no history; a moving light costs what
//! a still one does and lags nothing.
//!
//! Overload ([`Settings::omission`]): each tile's lights are sorted by a conservative
//! luminance bound with suffix sums; a pixel stops once the remaining lights' bound is
//! at most the fraction of the luminance it has accumulated, so omitted direct energy
//! never exceeds that fraction of the pixel's (local) luminance. Rank alone never
//! decides. A candidate only: not promoted.
//!
//! No production caller yet: the renderer has no opt-in wiring (the shared `Quality`
//! and lighting PSOs are outside this module); the G1 bench (`bench`, ignored tests)
//! drives it on a trace()-built G-buffer.
#![allow(dead_code)]

pub(crate) mod gpu;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

/// V2 shading settings (an opt-in A/B variant; nothing defaults to it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Settings {
    /// Fixed source samples per area light and receiver (spheres, discs and tubes on
    /// a golden-angle spiral, rects on the nearest square grid; ideal points use one).
    pub samples: u32,
    /// The omitted direct-energy fraction overload may drop per pixel (0.01: 1%);
    /// `None` shades every selected light.
    pub omission: Option<f32>,
    /// Ray-origin normal offset in pixel footprints at the receiver's distance.
    pub bias_footprints: f32,
    /// Smallest ray-origin normal offset, metres.
    pub bias_min: f32,
    /// Classify (tile, light) pairs against trace()'s instances (fully clear pairs trace
    /// nothing); `false` traces every pair through the whole TLAS (the A/B of the lever).
    pub classify: bool,
    /// Resolve named instances' visibility with per-triangle sample masks; `false` traces
    /// one segment per sample through them (the A/B of the masks).
    pub masks: bool,
    /// Bench diagnostics: per pixel, the first light whose sample masks disagree with
    /// per-sample rays, over the stats image (`LIGHTS_DEBUG`).
    pub debug_masks: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            samples: 16,
            omission: None,
            bias_footprints: 1.5,
            bias_min: 1e-4,
            classify: true,
            masks: true,
            debug_masks: false,
        }
    }
}
