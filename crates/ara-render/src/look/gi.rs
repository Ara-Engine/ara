//! Probe-volume global illumination for the raster renderer, without runtime rays.
//!
//! A one-time acquisition (a ray-traced bake where the GPU supports it, or a raster
//! cube capture) records what every probe sees: [`ProbeSurfaces`]. Each frame the
//! renderer relights those caches with the current sun, sky, local lights, emissive
//! surfaces and its own previous irradiance (multiple bounces), and shades the
//! G-buffer from the resulting irradiance volume. Lighting is deterministic: fixed
//! directions, no per-pixel or per-frame random sampling.
use anyhow::{Result, ensure};
use ara_gpu::{Access, Buffer};

/// Cache texels per probe (8x8 octahedral directions).
pub const SURFACE_TEXELS: usize = 64;
/// Static vectors per probe: 64 distance moments, relocation, diagnostics.
pub const STATIC_VECTORS: usize = 66;
/// Irradiance texels per probe (6x6 octahedral).
pub const IRRADIANCE_TEXELS: usize = 36;
/// Irradiance kept from the previous frame; relighting is noise-free, so this
/// only smooths changes (camera-dependent shadowing at cache hits).
pub(crate) const HYSTERESIS: f32 = 0.6;
/// Probes relit per frame; the rest carry their irradiance forward.
pub(crate) const BUDGET: u32 = 16384;

/// What each probe of a lattice sees, from a one-time acquisition.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeSurfaces {
    /// First probe centre in world metres.
    pub origin: [f32; 3],
    /// Probe separation per axis, in metres.
    pub spacing: [f32; 3],
    /// Lattice dimensions; probe `i` is at x = i % x, then y, then z.
    pub counts: [u32; 3],
    /// [`SURFACE_TEXELS`] per probe, octahedral row-major. Packed per texel: x
    /// diffuse albedo RGB8 and sky fraction 8 (low to high bits); y octahedral hit
    /// normal as two UNORM16; z emissive red and green as f16; w emissive blue and
    /// mean hit distance (metres) as f16. Albedo, normal, emissive and distance
    /// average the texel's geometry hits; the sky fraction is the share of misses.
    pub texels: Vec<[u32; 4]>,
    /// [`STATIC_VECTORS`] per probe: 64 octahedral (mean distance, mean squared
    /// distance) moments, then (relocation offset xyz, active 0/1), then
    /// acquisition diagnostics.
    pub statics: Vec<[f32; 4]>,
}

impl ProbeSurfaces {
    /// Probe count.
    pub fn len(&self) -> usize {
        self.counts.iter().map(|&n| n as usize).product()
    }
    /// A lattice always holds at least eight probes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Checks the lattice shape and finite data before upload.
    pub fn validate(&self) -> Result<()> {
        let n = self.len();
        ensure!(
            self.counts.iter().all(|&c| (2..=256).contains(&c)) && n <= 1 << 18,
            "Invalid probe lattice"
        );
        ensure!(
            self.origin
                .iter()
                .chain(&self.spacing)
                .all(|v| v.is_finite())
                && self.spacing.iter().all(|&s| s > 0.),
            "Invalid probe positions"
        );
        ensure!(
            self.texels.len() == n * SURFACE_TEXELS && self.statics.len() == n * STATIC_VECTORS,
            "Probe surfaces do not match the lattice"
        );
        ensure!(
            self.statics.iter().flatten().all(|v| v.is_finite()),
            "Probe statics must be finite"
        );
        Ok(())
    }
    /// Surface bias used when sampling the volume: a fifth of the smallest spacing,
    /// at most 15 cm.
    pub(crate) fn bias(&self) -> f32 {
        (self.spacing.iter().copied().fold(f32::INFINITY, f32::min) * 0.2).min(0.15)
    }
}

/// Uploaded probe volume. Irradiance ping-pongs: frames read `1 - slot` and write
/// `slot`.
pub(crate) struct GiGpu {
    pub origin: [f32; 3],
    pub spacing: [f32; 3],
    pub counts: [u32; 3],
    pub probes: u32,
    pub bias: f32,
    pub cache: Buffer,
    pub statics: Buffer,
    pub irradiance: [Buffer; 2],
    /// Bindless indices: cache, statics, irradiance 0, irradiance 1.
    pub indices: [u32; 4],
    pub access: [Access; 2],
    pub slot: usize,
    pub cursor: u32,
}

impl GiGpu {
    /// Fills the shader record's GI rows for this frame.
    pub(crate) fn write(&self, frame: &mut super::frame::LookFrame) {
        let [ox, oy, oz] = self.origin;
        let [sx, sy, sz] = self.spacing;
        let [cx, cy, cz] = self.counts;
        let read = self.indices[2 + (1 - self.slot)];
        let write = self.indices[2 + self.slot];
        frame.gi_origin = [ox, oy, oz, HYSTERESIS];
        frame.gi_spacing = [sx, sy, sz, self.bias];
        frame.gi_counts = [cx, cy, cz, self.probes];
        frame.gi_buffers = [self.indices[0], self.indices[1], read, write];
        frame.gi_update = [self.cursor, BUDGET.min(self.probes), 0, 0];
    }
    /// Advances the ping-pong slot and update window after a completed frame.
    pub(crate) fn advance(&mut self) {
        self.slot = 1 - self.slot;
        self.cursor = (self.cursor + BUDGET.min(self.probes)) % self.probes;
    }
}
