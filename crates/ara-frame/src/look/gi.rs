//! Probe-volume records. Relighting and the atlases stay in `ara-render`.
use anyhow::{Result, ensure};
mod layout;
pub use layout::{ProbeLattice, ProbeLayout, MAX_LATTICES};

/// Cache texels per probe (8x8 octahedral directions).
pub const SURFACE_TEXELS: usize = 64;
/// Static vectors per probe: [`MOMENT_VECTORS`] of packed distance moments,
/// relocation, diagnostics.
pub const STATIC_VECTORS: usize = MOMENT_VECTORS + 2;
/// Visibility texels per side of a probe's octahedral distance moments: twice the
/// cache's, as DDGI keeps its depth finer than its irradiance. Walls thinner than
/// the probe spacing need it: an 8x8 texel (some 22 degrees) next to a wall's edge
/// mixes the wall with the open space past it, which passes points behind the wall.
pub const MOMENT_SIDE: usize = 16;
/// Static vectors holding the moments: four packed texels each ([`pack_moments`]).
pub const MOMENT_VECTORS: usize = MOMENT_SIDE * MOMENT_SIDE / 4;

/// Distance (m) as eight bits of its base-2 logarithm, 4 mm to 65 km in steps of
/// about 7% (`giParallax`, look-gi.slangh, decodes it).
pub fn log_distance(metres: f32) -> u32 {
    ((metres.max(1e-6).log2() + 8.) / 24. * 255.)
        .round()
        .clamp(0., 255.) as u32
}

/// Distance (m) the moments of a lattice with `spacing` clamp each ray to, and
/// normalize by: twice its largest spacing, as DDGI clamps its ray distance (shading
/// weighs points within one cell of a probe, and a missed ray's far distance would
/// swamp a texel that also sees a near wall).
pub fn moment_limit(spacing: [f32; 3]) -> f32 {
    2. * spacing.iter().copied().fold(0., f32::max)
}

/// One visibility texel: its rays' mean distance and mean squared distance, each
/// over `limit` (and its square) as UNORM16, mean in the low bits. Moment vector
/// `v` of a probe holds the 16x16 texels (2x + i, 2y + j) for cache texel
/// (x, y) = (v % 8, v / 8) in component i + 2j, as f32 bit patterns (not numbers).
pub fn pack_moments(mean: f32, squared: f32, limit: f32) -> f32 {
    let unorm = |v: f32| (v.clamp(0., 1.) * 65535.).round() as u32;
    f32::from_bits(unorm(mean / limit) | unorm(squared / (limit * limit)) << 16)
}
/// What each probe of a pool sees, from a one-time acquisition. Equality compares
/// the statics bit for bit: packed moments are bit patterns, some of them NaN.
#[derive(Debug, Clone)]
pub struct ProbeSurfaces {
    /// The pool's lattices, finest first; probes numbered lattice after lattice.
    pub layout: ProbeLayout,
    /// [`SURFACE_TEXELS`] per probe, octahedral row-major. Packed per texel: x
    /// diffuse albedo RGB8 and sky fraction 8 (low to high bits); y the relit hit's
    /// octahedral normal as two UNORM10, its bake sub-ray (4 bits: (k % 4, k / 4) of the
    /// texel's 4x4) and the hits' mean distance ([`log_distance`], what reflections
    /// parallax-correct with); z emissive as RGB9E5; w a mask of the sub-rays (bit k)
    /// whose hits lie on the relit hit's plane (16 bits; relighting spreads the texel's
    /// light over those only), then the relit hit's distance along its sub-ray (metres,
    /// f16). The relit hit is a real one on the nearest surface the texel sees
    /// (ara-runtime-render `relit_hit`).
    /// Albedo and emissive average the texel's geometry hits; the sky fraction is the
    /// share of misses.
    pub texels: Vec<[u32; 4]>,
    /// [`STATIC_VECTORS`] per probe: [`MOMENT_VECTORS`] of 16x16 octahedral
    /// distance moments ([`pack_moments`]), then (relocation offset xyz, active
    /// 0/1), then acquisition diagnostics.
    pub statics: Vec<[f32; 4]>,
}

impl PartialEq for ProbeSurfaces {
    fn eq(&self, other: &Self) -> bool {
        fn bits(s: &ProbeSurfaces) -> &[u32] {
            bytemuck::cast_slice(&s.statics)
        }
        self.layout == other.layout && self.texels == other.texels && bits(self) == bits(other)
    }
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
        // Packed moments are bit patterns; the vectors after them are numbers.
        ensure!(
            self.statics
                .as_chunks::<STATIC_VECTORS>()
                .0
                .iter()
                .flat_map(|probe| probe[MOMENT_VECTORS..].iter().flatten())
                .all(|v| v.is_finite()),
            "Probe statics must be finite"
        );
        Ok(())
    }
}

/// Octahedral decode of `p` in 0..1 (`octDecode`, raster.slangh).
pub(crate) fn oct_decode(p: glam::Vec2) -> glam::Vec3 {
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

