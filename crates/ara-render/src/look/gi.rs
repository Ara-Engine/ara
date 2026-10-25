//! Probe-volume global illumination for the raster renderer, without runtime rays.
//!
//! A one-time acquisition (a ray-traced bake where the GPU supports it, or a raster
//! cube capture) records what every probe sees: [`ProbeSurfaces`]. Each frame the
//! renderer relights those caches with the current sun, sky, local lights, emissive
//! surfaces and its own previous irradiance (multiple bounces), and shades the
//! G-buffer from the resulting irradiance volume. Lighting is deterministic: fixed
//! directions, no per-pixel or per-frame random sampling.
use anyhow::{Result, ensure};
use ara_gpu::{Access, Buffer, Image};

/// Cache texels per probe (8x8 octahedral directions).
pub const SURFACE_TEXELS: usize = 64;
/// Static vectors per probe: 64 distance moments, relocation, diagnostics.
pub const STATIC_VECTORS: usize = 66;
/// Irradiance kept from the previous frame; relighting is noise-free, so this
/// only smooths changes (camera-dependent shadowing at cache hits).
pub(crate) const HYSTERESIS: f32 = 0.6;
/// Frames that relight every probe without hysteresis, each adding one full
/// bounce: after a capture reset (whose exposure re-meters as well, so captures
/// converge within their settle frames whatever the preset's budget) and once a
/// new volume's records have all landed.
pub(crate) const CONVERGE_FRAMES: u32 = 8;
/// Frames over which a newly converged volume fades into shading.
const FADE_FRAMES: f32 = 30.;

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
}

/// Bytes of one probe record in a [`ProbeUpdate`]: [probe id, 0, 0, 0], then its
/// [`SURFACE_TEXELS`] cache texels and [`STATIC_VECTORS`] statics.
pub const PROBE_RECORD: usize = 16 + (SURFACE_TEXELS + STATIC_VECTORS) * 16;

/// Probe surfaces packed for the renderer: the lattice and one record per probe
/// to write. It is plain data, so build it off the render thread; applying it is
/// one upload and a compute scatter (`Renderer::update_probe_surfaces`), and a
/// large update can be applied a range of records at a time
/// (`Renderer::update_probe_records`); each probe appears once per update, so
/// its ranges apply in any order.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeUpdate {
    /// First probe centre in world metres.
    pub origin: [f32; 3],
    /// Probe separation per axis, in metres.
    pub spacing: [f32; 3],
    /// Lattice dimensions.
    pub counts: [u32; 3],
    /// Packed records, [`PROBE_RECORD`] bytes each.
    pub records: Vec<u8>,
}

impl ProbeUpdate {
    /// Records `probes` of `surfaces` (`None`: every probe).
    pub fn new(surfaces: &ProbeSurfaces, probes: Option<&[u32]>) -> Result<Self> {
        surfaces.validate()?;
        let all: Vec<u32>;
        let probes = match probes {
            Some(probes) => probes,
            None => {
                all = (0..surfaces.len() as u32).collect();
                &all
            }
        };
        let mut records = Vec::with_capacity(probes.len() * PROBE_RECORD);
        for &id in probes {
            let p = id as usize;
            ensure!(p < surfaces.len(), "probe {id} outside the lattice");
            records.extend_from_slice(bytemuck::bytes_of(&[id, 0, 0, 0]));
            records.extend_from_slice(bytemuck::cast_slice(
                &surfaces.texels[p * SURFACE_TEXELS..(p + 1) * SURFACE_TEXELS],
            ));
            records.extend_from_slice(bytemuck::cast_slice(
                &surfaces.statics[p * STATIC_VECTORS..(p + 1) * STATIC_VECTORS],
            ));
        }
        Ok(Self {
            origin: surfaces.origin,
            spacing: surfaces.spacing,
            counts: surfaces.counts,
            records,
        })
    }

    /// Records in this update.
    pub fn len(&self) -> usize {
        self.records.len() / PROBE_RECORD
    }

    /// Whether the update writes no probe.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Probes in the lattice.
    pub fn probes(&self) -> usize {
        self.counts.iter().map(|&n| n as usize).product()
    }

    /// Checks the lattice and the record framing before upload.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.counts.iter().all(|&c| (2..=256).contains(&c)) && self.probes() <= 1 << 18,
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
            self.records.len().is_multiple_of(PROBE_RECORD),
            "Probe records are not whole"
        );
        Ok(())
    }

    /// Surface bias used when sampling the volume: a fifth of the smallest spacing,
    /// at most 15 cm.
    pub(crate) fn bias(&self) -> f32 {
        (self.spacing.iter().copied().fold(f32::INFINITY, f32::min) * 0.2).min(0.15)
    }
}

/// Probes per atlas row: a near-square atlas for any lattice.
pub(crate) fn atlas_width(probes: usize) -> u32 {
    (probes as f64).sqrt().ceil() as u32
}

/// Probes per scatter dispatch (one workgroup each).
pub(crate) const DISPATCH_PROBES: u32 = 65_535;

/// Host staging holding probe records until a frame scatters them
/// (`LookGpu::probe_writes`, then back to `LookGpu::probe_staging`).
pub(crate) struct ProbeStaging {
    pub buffer: Buffer,
    pub capacity: usize,
    /// Storage index of `buffer`.
    pub index: u32,
}

/// Uploaded probe volume. Irradiance atlases ping-pong: frames read `1 - slot`
/// and write `slot`.
pub(crate) struct GiGpu {
    pub origin: [f32; 3],
    pub spacing: [f32; 3],
    pub counts: [u32; 3],
    pub probes: u32,
    pub bias: f32,
    /// Probes per atlas row.
    pub atlas: u32,
    pub cache: Buffer,
    /// Relocation offset and active flag per probe.
    pub meta: Buffer,
    /// Static distance moments (bordered 10x10 tiles).
    pub moments: Image,
    /// Relit irradiance (bordered 8x8 tiles), ping-ponged.
    pub irradiance: [Image; 2],
    /// Bindless indices: cache, meta (buffers), moments (sampled).
    pub indices: [u32; 3],
    pub irradiance_sampled: [u32; 2],
    pub irradiance_storage: [u32; 2],
    /// Storage index of `moments` (in-place edits, `giScatterMain`).
    pub moments_storage: u32,
    pub access: [Access; 2],
    /// Not yet cleared: the next frame imports it undefined and runs `GI.Clear`.
    pub fresh: bool,
    pub slot: usize,
    pub cursor: u32,
    /// Remaining [`CONVERGE_FRAMES`].
    pub converge: u32,
    /// Records written since the volume was created; a new lattice stays out of
    /// shading until every probe's has landed and the volume has converged.
    pub received: usize,
    /// Shading weight: 0 while a new volume fills and converges (the ambient
    /// stays as it was), then rising to 1 over [`FADE_FRAMES`].
    pub weight: f32,
}

impl GiGpu {
    /// Counts `records` written; once every probe of a new volume has landed it
    /// starts converging (still out of shading).
    pub(crate) fn received(&mut self, records: usize) {
        if self.weight > 0. || self.converge > 0 {
            return;
        }
        self.received += records;
        if self.received >= self.probes as usize {
            self.converge = CONVERGE_FRAMES;
        }
    }
    /// Probes relit this frame: `budget`, or all of them while converging.
    fn window(&self, budget: u32) -> u32 {
        if self.converge > 0 {
            self.probes
        } else {
            budget.min(self.probes)
        }
    }
    /// Fills the shader record's GI rows for this frame (`budget` probes relit).
    pub(crate) fn write(&self, frame: &mut super::frame::LookFrame, budget: u32) {
        let [ox, oy, oz] = self.origin;
        let [sx, sy, sz] = self.spacing;
        let [cx, cy, cz] = self.counts;
        let (read, write) = (1 - self.slot, self.slot);
        let hysteresis = if self.converge > 0 { 0. } else { HYSTERESIS };
        frame.gi_origin = [ox, oy, oz, hysteresis];
        frame.gi_spacing = [sx, sy, sz, self.bias];
        frame.gi_counts = [cx, cy, cz, self.probes];
        frame.gi_buffers = [self.indices[0], self.indices[1], 0, 0];
        frame.gi_update = [
            self.cursor,
            self.window(budget),
            self.atlas,
            self.weight.to_bits(),
        ];
        frame.gi_images = [
            self.irradiance_sampled[read],
            self.irradiance_sampled[write],
            self.irradiance_storage[write],
            self.indices[2],
        ];
    }
    /// Advances the ping-pong slot and update window after a completed frame.
    pub(crate) fn advance(&mut self, budget: u32) {
        self.slot = 1 - self.slot;
        self.cursor = (self.cursor + self.window(budget)) % self.probes;
        if self.converge > 0 {
            self.converge -= 1;
        } else if self.received >= self.probes as usize {
            self.weight = (self.weight + 1. / FADE_FRAMES).min(1.);
        }
    }
    /// Shows the volume at once (capture resets converge it within their frames).
    pub(crate) fn show(&mut self) {
        self.received = self.probes as usize;
        self.weight = 1.;
    }
}
