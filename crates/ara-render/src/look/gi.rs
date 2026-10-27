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
mod layout;
pub use layout::{MAX_LATTICES, ProbeLattice, ProbeLayout};

/// Cache texels per probe (8x8 octahedral directions).
pub const SURFACE_TEXELS: usize = 64;
/// Static vectors per probe: 64 distance moments, relocation, diagnostics.
pub const STATIC_VECTORS: usize = 66;
/// Irradiance kept from the previous frame; relighting is noise-free, so this
/// only smooths changes (camera-dependent shadowing at cache hits).
pub(crate) const HYSTERESIS: f32 = 0.6;
/// Full-volume bounces without hysteresis after new records or a capture reset.
/// Live initialization spreads each bounce over a bounded initial budget; a capture
/// relights the full volume per frame and re-meters exposure within its settle.
pub(crate) const CONVERGE_FRAMES: u32 = 8;
/// Initial relight work, before readiness; steady frames keep the preset budget.
const INITIAL_PROBES: u32 = 4096;
/// Frames over which a newly converged volume fades into shading.
const FADE_FRAMES: f32 = 30.;
/// Cells over which a finer lattice fades into the next across its edge: full
/// weight three cells inside its outermost probes (`look-gi.slangh`).
const FINE_FALLOFF: f32 = 3.5;
/// The coarsest lattice fades to the sky over half a cell past its outermost probes.
const COARSE_FALLOFF: f32 = 0.5;

/// What each probe of a pool sees, from a one-time acquisition.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeSurfaces {
    /// The pool's lattices, finest first; probes numbered lattice after lattice.
    pub layout: ProbeLayout,
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
    /// Allocation layout, independent of the acquired texels and static records.
    pub fn layout(&self) -> ProbeLayout {
        self.layout
    }
    /// Probe count.
    pub fn len(&self) -> usize {
        self.layout.probe_count()
    }
    /// A lattice always holds at least eight probes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Checks the lattice shape and finite data before upload.
    pub fn validate(&self) -> Result<()> {
        self.layout().validate()?;
        let n = self.len();
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

/// Power of the cosine lobe the distance moments are filtered over: about one 8x8
/// octahedral texel (some 22 degrees) across, as DDGI filters its depth maps. Raw
/// per-texel moments jump at texel boundaries, so Chebyshev visibility flips where
/// the direction to a shading point crosses one: sharp straight lines across flat
/// faces near a probe.
const MOMENT_LOBE: i32 = 12;

/// Per moment texel, the normalized cosine-lobe weights of all 64 texels; the
/// mixture of the texels' (mean, mean squared) keeps variance meaningful.
fn moment_filter() -> &'static [[f32; SURFACE_TEXELS]; SURFACE_TEXELS] {
    static FILTER: std::sync::OnceLock<[[f32; SURFACE_TEXELS]; SURFACE_TEXELS]> =
        std::sync::OnceLock::new();
    FILTER.get_or_init(|| {
        let directions: [glam::Vec3; SURFACE_TEXELS] = std::array::from_fn(|t| {
            oct_decode((glam::Vec2::new((t % 8) as f32, (t / 8) as f32) + 0.5) / 8.)
        });
        std::array::from_fn(|t| {
            let mut weights: [f32; SURFACE_TEXELS] =
                std::array::from_fn(|u| directions[t].dot(directions[u]).max(0.).powi(MOMENT_LOBE));
            let total: f32 = weights.iter().sum();
            weights.iter_mut().for_each(|w| *w /= total);
            weights
        })
    })
}

/// Octahedral decode of `p` in 0..1 (`octDecode`, raster.slangh).
fn oct_decode(p: glam::Vec2) -> glam::Vec3 {
    let p = p * 2. - 1.;
    let mut n = glam::Vec3::new(p.x, p.y, 1. - p.x.abs() - p.y.abs());
    let t = (-n.z).clamp(0., 1.);
    n.x += if n.x >= 0. { -t } else { t };
    n.y += if n.y >= 0. { -t } else { t };
    n.normalize()
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
    /// The pool's lattices, finest first.
    pub layout: ProbeLayout,
    /// Packed records, [`PROBE_RECORD`] bytes each.
    pub records: Vec<u8>,
}

impl ProbeUpdate {
    /// Allocation layout; available before the first records are uploaded.
    pub fn layout(&self) -> ProbeLayout {
        self.layout
    }
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
        let filter = moment_filter();
        let (lattices, bases) = (surfaces.layout.lattices(), surfaces.layout.bases());
        for &id in probes {
            let p = id as usize;
            ensure!(p < surfaces.len(), "probe {id} outside the lattice");
            records.extend_from_slice(bytemuck::bytes_of(&[id, 0, 0, 0]));
            records.extend_from_slice(bytemuck::cast_slice(
                &surfaces.texels[p * SURFACE_TEXELS..(p + 1) * SURFACE_TEXELS],
            ));
            let mut statics: [[f32; 4]; STATIC_VECTORS] = surfaces.statics
                [p * STATIC_VECTORS..(p + 1) * STATIC_VECTORS]
                .try_into()
                .expect("a probe's statics slice has STATIC_VECTORS entries");
            // Clamped to twice the probe's lattice spacing, as DDGI clamps its ray
            // distance: a missed ray's far distance must not pull its neighbours'
            // means past every shading point (light leaks at silhouettes). Shading
            // only weighs points within one cell of the probe.
            let lattice = (0..lattices.len())
                .rev()
                .find(|&i| bases[i] <= p)
                .map_or(lattices[0], |i| lattices[i]);
            let limit = 2. * lattice.spacing.iter().copied().fold(0., f32::max);
            let raw: [[f32; 4]; SURFACE_TEXELS] = std::array::from_fn(|t| {
                let [mean, squared, ..] = statics[t];
                [mean.min(limit), squared.min(limit * limit), 0., 0.]
            });
            for (texel, weights) in statics.iter_mut().zip(filter) {
                let (mean, squared) = weights
                    .iter()
                    .zip(&raw)
                    .fold((0., 0.), |(m, s), (w, r)| (m + w * r[0], s + w * r[1]));
                texel[0] = mean;
                texel[1] = squared;
            }
            records.extend_from_slice(bytemuck::cast_slice(&statics));
        }
        Ok(Self {
            layout: surfaces.layout,
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

    /// Probes in the pool.
    pub fn probes(&self) -> usize {
        self.layout.probe_count()
    }

    /// Whether every record is of a pool's finer lattices: a staged delivery's
    /// later part, which may land after the volume is ready.
    pub fn finer_only(&self) -> bool {
        let finer = self.layout.bases()[self.layout.lattices().len() - 1] as u32;
        finer > 0 && self.ids(0..self.len()).all(|id| id < finer)
    }

    /// Probe ids of `records` (a range of [`Self::len`]).
    pub fn ids(&self, records: std::ops::Range<usize>) -> impl Iterator<Item = u32> + '_ {
        records.map(|r| {
            bytemuck::pod_read_unaligned(&self.records[r * PROBE_RECORD..r * PROBE_RECORD + 4])
        })
    }

    /// Checks the lattice and the record framing before upload.
    pub fn validate(&self) -> Result<()> {
        self.layout().validate()?;
        ensure!(
            self.records.len().is_multiple_of(PROBE_RECORD),
            "Probe records are not whole"
        );
        Ok(())
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

/// Uploaded probe pool. Irradiance atlases ping-pong: frames read `1 - slot`
/// and write `slot`.
pub(crate) struct GiGpu {
    pub layout: ProbeLayout,
    pub probes: u32,
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
    pub surface_access: [Access; 3],
    pub access: [Access; 2],
    /// Initialization is incomplete; clear a bounded range before accepting data.
    pub fresh: bool,
    pub clear_cursor: u32,
    pub clear_budget: u32,
    pub slot: usize,
    pub cursor: u32,
    /// The previous frame's relit window (first probe, count). The atlas this
    /// frame writes last held every probe two frames ago, so only that window
    /// is stale in it: the relight copies it forward rather than the whole
    /// pool. Zero whenever both atlases hold the same irradiance.
    pub relit: (u32, u32),
    /// Remaining full-volume convergence bounces.
    pub converge: u32,
    pub converge_progress: u32,
    pub capture_convergence: bool,
    /// Frames are not shown (`Renderer::set_unseen`): each bounce relights its whole
    /// range in one frame, as captures do, and a converged volume shows at once
    /// instead of fading in.
    pub unseen: bool,
    pub relit_once: bool,
    /// Records of probes `deferred..` (the coarsest lattice) written since the
    /// volume was created; it stays out of shading until they have all landed and
    /// it has converged.
    pub received: usize,
    /// Shading weight: 0 while a new volume fills and converges (the ambient
    /// stays as it was), then rising to 1 over [`FADE_FRAMES`].
    pub weight: f32,
    /// Probes `0..deferred`, a pool's finer lattices, may land after the
    /// coarsest: the volume is ready on the coarsest alone, and they converge
    /// and fade in afterwards. Zero for a single lattice.
    pub deferred: u32,
    /// Records of the deferred probes written since the volume was created.
    pub deferred_received: usize,
    /// Deferred convergence has begun (once the coarsest lattice converged).
    pub deferred_started: bool,
    /// Remaining bounces over the deferred probes, at the preset budget.
    pub deferred_converge: u32,
    pub deferred_progress: u32,
    /// Shading weight of the finer lattices: 0 until they have landed and
    /// converged, then rising to 1 over [`FADE_FRAMES`] (a cross-fade from the
    /// coarsest lattice beneath them).
    pub deferred_weight: f32,
}

impl GiGpu {
    /// Whether every probe the volume needs, the coarsest lattice's, has landed.
    pub(crate) fn landed(&self) -> bool {
        self.received + self.deferred as usize >= self.probes as usize
    }

    /// Whether the finer lattices have landed but not yet faded fully in.
    pub(crate) fn deferred_arriving(&self) -> bool {
        self.deferred > 0
            && self.deferred_received >= self.deferred as usize
            && self.deferred_weight < 1.
    }

    /// Counts the records written for probes `ids`. Once the coarsest lattice of
    /// a new volume has landed it starts converging (still out of shading); the
    /// finer lattices are counted apart and converge after it.
    pub(crate) fn received(&mut self, ids: impl Iterator<Item = u32>) {
        let (mut coarse, mut fine) = (0, 0);
        for id in ids {
            if id < self.deferred {
                fine += 1;
            } else {
                coarse += 1;
            }
        }
        if !self.deferred_started {
            self.deferred_received += fine;
        }
        if self.weight > 0. || self.converge > 0 {
            return;
        }
        self.received += coarse;
        if self.landed() {
            self.converge = CONVERGE_FRAMES;
            self.converge_progress = 0;
            self.cursor = self.range().0;
        }
    }

    /// Probes relit this phase: the coarsest lattice while it converges, the
    /// finer lattices while they do, else the whole pool.
    fn range(&self) -> (u32, u32) {
        if self.converge > 0 && !self.capture_convergence {
            (self.deferred, self.probes)
        } else if self.deferred_converge > 0 {
            (0, self.deferred)
        } else {
            (0, self.probes)
        }
    }

    /// This frame's relit window (first probe, count). Captures retain full-grid
    /// bounces, and unseen frames relight each converging range whole; live
    /// initialization uses a bounded window over the coarsest lattice, and the finer
    /// lattices converge at the preset budget. A window never runs past its range's
    /// end (the next one wraps).
    fn window(&self, budget: u32) -> (u32, u32) {
        let (start, end) = self.range();
        let converging = self.converge > 0 || self.deferred_converge > 0;
        let window = if (self.converge > 0 && self.capture_convergence)
            || (self.unseen && converging && budget > 0)
        {
            self.probes
        } else if self.converge > 0 && budget > 0 {
            budget.max(INITIAL_PROBES)
        } else {
            budget
        };
        let cursor = self.cursor.clamp(start, end);
        (cursor, window.min(end - cursor))
    }

    /// Relight workgroups this frame: one per relit probe, then one per probe
    /// of the previous window to copy forward. Proportional to the budget, not
    /// to the pool.
    pub(crate) fn relight_groups(&self, budget: u32) -> u32 {
        self.window(budget).1 + self.relit.1
    }
    /// Fills the shader record's GI rows for this frame (`budget` probes relit).
    pub(crate) fn write(&self, frame: &mut super::frame::LookFrame, budget: u32) {
        if self.clearing_in_slices() {
            return;
        }
        let (read, write) = (1 - self.slot, self.slot);
        let converging = self.converge > 0 || self.deferred_converge > 0;
        let hysteresis = if converging { 0. } else { HYSTERESIS };
        let finer = if self.deferred > 0 {
            self.deferred_weight
        } else {
            1.
        };
        frame.gi_params = [hysteresis, finer, 0., 0.];
        let lattices = self.layout.lattices();
        frame.gi_counts = [
            lattices.len() as u32,
            self.relit.0,
            self.relit.1,
            self.probes,
        ];
        let bases = self.layout.bases();
        for (i, lattice) in lattices.iter().enumerate() {
            let falloff = if i + 1 == lattices.len() {
                COARSE_FALLOFF
            } else {
                FINE_FALLOFF
            };
            let ([ox, oy, oz], [sx, sy, sz]) = (lattice.origin, lattice.spacing);
            let [cx, cy, cz] = lattice.counts;
            frame.gi_lattice_origin[i] = [ox, oy, oz, falloff];
            frame.gi_lattice_spacing[i] = [sx, sy, sz, lattice.bias()];
            frame.gi_lattice_counts[i] = [cx, cy, cz, bases[i] as u32];
        }
        frame.gi_buffers = [self.indices[0], self.indices[1], 0, 0];
        let (cursor, window) = self.window(budget);
        frame.gi_update = [cursor, window, self.atlas, self.weight.to_bits()];
        frame.gi_images = [
            self.irradiance_sampled[read],
            self.irradiance_sampled[write],
            self.irradiance_storage[write],
            self.indices[2],
        ];
    }
    /// Advances the ping-pong slot and update window after a completed frame.
    pub(crate) fn advance(&mut self, budget: u32) {
        if self.clearing_in_slices() {
            return;
        }
        let (cursor, window) = self.window(budget);
        let (start, end) = self.range();
        self.relit_once = true;
        self.relit = (cursor, window);
        self.slot = 1 - self.slot;
        self.cursor = cursor + window;
        if self.cursor >= end {
            self.cursor = start;
        }
        if self.converge > 0 {
            self.converge_progress += window;
            if self.converge_progress >= end - start {
                self.converge_progress -= end - start;
                self.converge -= 1;
            }
            return;
        }
        // Unseen frames show a converged volume at once: no fade to watch.
        let fade = if self.unseen { 1. } else { 1. / FADE_FRAMES };
        if self.landed() {
            self.weight = (self.weight + fade).min(1.);
        }
        if self.deferred_converge > 0 {
            self.deferred_progress += window;
            if self.deferred_progress >= end - start {
                self.deferred_progress -= end - start;
                self.deferred_converge -= 1;
                if self.deferred_converge == 0 {
                    self.cursor = 0;
                }
            }
        } else if self.deferred_started {
            self.deferred_weight = (self.deferred_weight + fade).min(1.);
        } else if self.deferred > 0 && self.deferred_received >= self.deferred as usize {
            // The finer lattices landed after the coarsest converged: relight them
            // alone, at the preset budget, before they fade in.
            self.deferred_started = true;
            self.deferred_converge = CONVERGE_FRAMES;
            self.deferred_progress = 0;
            self.cursor = 0;
        }
    }

    pub(crate) fn clearing_in_slices(&self) -> bool {
        self.fresh && self.clear_budget < self.probes
    }

    pub(crate) fn clear_window(&self) -> (u32, u32) {
        (
            self.clear_cursor,
            (self.probes - self.clear_cursor).min(self.clear_budget),
        )
    }

    pub(crate) fn cleared(&mut self) {
        if self.fresh {
            self.clear_cursor += self.clear_window().1;
            self.fresh = self.clear_cursor < self.probes;
        }
    }
    /// Shows the volume at once (capture resets converge it within their frames).
    /// A new volume is still converging or fading into shading: the scene's
    /// brightness is changing under exposure (which then tracks its meter).
    pub(crate) fn arriving(&self) -> bool {
        self.converge > 0 || self.weight < 1.
    }

    pub(crate) fn show(&mut self) {
        self.received = (self.probes - self.deferred) as usize;
        self.weight = 1.;
        if self.deferred_received >= self.deferred as usize {
            self.deferred_started = true;
            self.deferred_converge = 0;
            self.deferred_weight = 1.;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments_are_filtered_over_a_cosine_lobe_and_clamped_to_the_lattice() -> Result<()> {
        let layout = ProbeLayout::single(ProbeLattice {
            origin: [0.; 3],
            spacing: [1.; 3],
            counts: [2; 3],
        });
        let probes = 8;
        let mut statics = vec![[0.; 4]; probes * STATIC_VECTORS];
        for p in 0..probes {
            for t in 0..SURFACE_TEXELS {
                // Probe 0: near geometry in half its texels, missed rays in the rest.
                statics[p * STATIC_VECTORS + t] = match (p, t < 32) {
                    (0, true) => [0.5, 0.25, 0., 0.],
                    (0, false) => [200., 40_000., 0., 0.],
                    _ => [1., 1., 0., 0.],
                };
            }
            statics[p * STATIC_VECTORS + SURFACE_TEXELS] = [0., 0., 0., 1.];
        }
        let surfaces = ProbeSurfaces {
            layout,
            texels: vec![[0; 4]; probes * SURFACE_TEXELS],
            statics,
        };
        let update = ProbeUpdate::new(&surfaces, None)?;
        let moments = |p: usize| -> Vec<[f32; 4]> {
            let at = p * PROBE_RECORD + 16 + SURFACE_TEXELS * 16;
            bytemuck::cast_slice(&update.records[at..at + SURFACE_TEXELS * 16]).to_vec()
        };
        // A uniform probe keeps its moments.
        assert!(
            moments(1)
                .iter()
                .all(|m| (m[0] - 1.).abs() < 1e-5 && (m[1] - 1.).abs() < 1e-5)
        );
        // Misses clamp to twice the spacing, and texels near the boundary blend both
        // sides instead of jumping between them.
        let near_far = moments(0);
        assert!(
            near_far
                .iter()
                .all(|m| (0.5 - 1e-5..=2. + 1e-5).contains(&m[0]))
        );
        assert!(near_far.iter().any(|m| m[0] > 0.6 && m[0] < 1.9));
        // Relocation and the active flag pass through.
        let at = PROBE_RECORD + 16 + (SURFACE_TEXELS + SURFACE_TEXELS) * 16;
        let active: [f32; 4] = bytemuck::pod_read_unaligned(&update.records[at..at + 16]);
        assert_eq!(active, [0., 0., 0., 1.]);
        Ok(())
    }
}
