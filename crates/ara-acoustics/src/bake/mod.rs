//! Baked reverb probes per world cell.
//!
//! A baker ([`GeometricBaker`] always; [`SteamBaker`] when Steam Audio is installed)
//! computes per-probe reverberation times and room statistics. [`AcousticCell`] is the
//! cooked, checksummed binary for one `ara-world` cell (same `x/z/slot` key), stored
//! beside the cell's pack data and decoded on a streaming worker. [`ProbeField`] holds
//! the resident cells and interpolates probes for the listener.

mod geometric;
mod steam;

pub use geometric::GeometricBaker;
pub use steam::{SteamBakeSettings, SteamBaker};

use crate::{
    geometry::MeshScene,
    math::{V3, distance},
};
use ara_audio::{AudioError, Result};
use std::collections::BTreeMap;

/// Baked acoustics at one probe position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbProbe {
    /// World position.
    pub position: V3,
    /// Low/mid/high reverberation time in seconds.
    pub rt60: [f32; 3],
    /// Mean free path in meters (room size).
    pub mean_free_path: f32,
    /// Visible air volume estimate in m³.
    pub volume: f32,
    /// Effective low/mid/high absorption, including escaping (open-sky) energy.
    pub absorption: [f32; 3],
    /// Fraction of probe rays that escape to open space.
    pub escape: f32,
}

impl ReverbProbe {
    const FLOATS: usize = 12;

    fn floats(&self) -> [f32; Self::FLOATS] {
        let p = self;
        [
            p.position[0],
            p.position[1],
            p.position[2],
            p.rt60[0],
            p.rt60[1],
            p.rt60[2],
            p.mean_free_path,
            p.volume,
            p.absorption[0],
            p.absorption[1],
            p.absorption[2],
            p.escape,
        ]
    }

    fn from_floats(f: [f32; Self::FLOATS]) -> Self {
        Self {
            position: [f[0], f[1], f[2]],
            rt60: [f[3], f[4], f[5]],
            mean_free_path: f[6],
            volume: f[7],
            absorption: [f[8], f[9], f[10]],
            escape: f[11],
        }
    }

    /// Reverberant-to-direct energy ratio at 1 m from the probe's statistics.
    pub fn level(&self) -> f32 {
        let surface = 4.0 * self.volume / self.mean_free_path.max(1e-3);
        let alpha = self.absorption[1].clamp(1e-3, 0.999);
        (16.0 * std::f32::consts::PI * (1.0 - alpha) / (surface * alpha).max(1e-3)).clamp(0.01, 4.0)
    }
}

/// `ara-world` cell address (`CellKey`): grid X/Z and layer slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellKey {
    /// Signed horizontal grid coordinate.
    pub x: i32,
    /// Signed depth grid coordinate.
    pub z: i32,
    /// Layer slot.
    pub slot: u32,
}

/// Cooked acoustics for one world cell.
#[derive(Debug, Clone, PartialEq)]
pub struct AcousticCell {
    /// The cell this data belongs to.
    pub key: CellKey,
    /// Baker identity and version (`ara-geometric/1`, `steam-audio/4.8.1`).
    pub baker: String,
    /// Probes, in bake order.
    pub probes: Vec<ReverbProbe>,
}

const MAGIC: &[u8; 8] = b"ARAACU01";
const VERSION: u32 = 1;
const MAX_PROBES: usize = 1 << 20;

impl AcousticCell {
    /// Deterministic little-endian bytes followed by a BLAKE3 checksum.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.baker.len() > 64 || self.probes.len() > MAX_PROBES {
            return Err(AudioError::Invalid(
                "acoustic cell baker name or probe count",
            ));
        }
        let mut out = Vec::with_capacity(40 + self.probes.len() * ReverbProbe::FLOATS * 4);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.key.x.to_le_bytes());
        out.extend_from_slice(&self.key.z.to_le_bytes());
        out.extend_from_slice(&self.key.slot.to_le_bytes());
        out.extend_from_slice(&(self.baker.len() as u16).to_le_bytes());
        out.extend_from_slice(self.baker.as_bytes());
        out.extend_from_slice(&(self.probes.len() as u32).to_le_bytes());
        for probe in &self.probes {
            for f in probe.floats() {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
        let hash = blake3::hash(&out);
        out.extend_from_slice(hash.as_bytes());
        Ok(out)
    }

    /// Validates framing, checksum and values before trusting any field.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let invalid = || AudioError::Codec("invalid acoustic cell".into());
        if bytes.len() < MAGIC.len() + 32 || &bytes[..8] != MAGIC {
            return Err(invalid());
        }
        let (body, hash) = bytes.split_at(bytes.len() - 32);
        if blake3::hash(body).as_bytes() != hash {
            return Err(AudioError::Codec("acoustic cell checksum mismatch".into()));
        }
        let mut cursor = 8;
        let mut take = |n: usize| -> Result<&[u8]> {
            let slice = body.get(cursor..cursor + n).ok_or_else(invalid)?;
            cursor += n;
            Ok(slice)
        };
        let u32_at = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if u32_at(take(4)?) != VERSION {
            return Err(AudioError::Codec(
                "unsupported acoustic cell version".into(),
            ));
        }
        let x = u32_at(take(4)?) as i32;
        let z = u32_at(take(4)?) as i32;
        let slot = u32_at(take(4)?);
        let name_len = take(2)?;
        let name_len = usize::from(u16::from_le_bytes([name_len[0], name_len[1]]));
        let baker = std::str::from_utf8(take(name_len)?)
            .map_err(|_| invalid())?
            .to_owned();
        let count = u32_at(take(4)?) as usize;
        if count > MAX_PROBES {
            return Err(invalid());
        }
        let mut probes = Vec::with_capacity(count);
        for _ in 0..count {
            let raw = take(ReverbProbe::FLOATS * 4)?;
            let floats: [f32; ReverbProbe::FLOATS] = std::array::from_fn(|i| {
                f32::from_le_bytes([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]])
            });
            if floats.iter().any(|f| !f.is_finite()) {
                return Err(invalid());
            }
            probes.push(ReverbProbe::from_floats(floats));
        }
        if cursor != body.len() {
            return Err(invalid());
        }
        Ok(Self {
            key: CellKey { x, z, slot },
            baker,
            probes,
        })
    }
}

/// Computes reverb probes for a scene. Runs offline (cook workers), never per frame.
pub trait ReverbBaker {
    /// Baker identity and version, recorded in cooked cells.
    fn name(&self) -> String;
    /// Bakes one probe per position, in order.
    fn bake(&mut self, scene: &MeshScene, positions: &[V3]) -> Result<Vec<ReverbProbe>>;
}

/// Probe positions on a horizontal grid over `min..max` (X/Z), `height` above the
/// first floor found below each grid point. Deterministic row-major order.
pub fn floor_probes(scene: &MeshScene, min: V3, max: V3, spacing: f32, height: f32) -> Vec<V3> {
    let mut out = Vec::new();
    if !spacing.is_finite() || spacing <= 0.0 {
        return out;
    }
    let nx = ((max[0] - min[0]) / spacing).floor().max(0.0) as usize + 1;
    let nz = ((max[2] - min[2]) / spacing).floor().max(0.0) as usize + 1;
    for iz in 0..nz.min(4096) {
        for ix in 0..nx.min(4096) {
            let top = [
                min[0] + ix as f32 * spacing,
                max[1] - 0.01,
                min[2] + iz as f32 * spacing,
            ];
            if let Some(hit) = scene.closest(top, [0.0, -1.0, 0.0], max[1] - min[1]) {
                let floor_y = top[1] - hit.distance;
                let y = (floor_y + height).min(max[1] - 0.05);
                out.push([top[0], y, top[2]]);
            }
        }
    }
    out
}

/// Interpolated probe data for one position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeSample {
    /// Blended reverberation times.
    pub rt60: [f32; 3],
    /// Blended mean free path.
    pub mean_free_path: f32,
    /// Blended reverberant level at 1 m.
    pub level: f32,
    /// Distance to the nearest contributing probe.
    pub nearest: f32,
}

/// Resident baked cells; load and unload follow `ara-world` streaming.
#[derive(Debug, Default)]
pub struct ProbeField {
    cells: BTreeMap<CellKey, AcousticCell>,
}

impl ProbeField {
    /// An empty field.
    pub fn new() -> Self {
        Self::default()
    }
    /// Makes a decoded cell resident, replacing an older bake of the same key.
    pub fn load(&mut self, cell: AcousticCell) {
        self.cells.insert(cell.key, cell);
    }
    /// Drops a cell when the world streams it out.
    pub fn unload(&mut self, key: CellKey) -> Option<AcousticCell> {
        self.cells.remove(&key)
    }
    /// Resident probe count.
    pub fn probes(&self) -> usize {
        self.cells.values().map(|c| c.probes.len()).sum()
    }

    /// Inverse-square-distance blend of up to four probes within `radius` of `point`
    /// that pass `accept` (e.g. same zone). Ties order by cell key and index.
    pub fn sample(
        &self,
        point: V3,
        radius: f32,
        accept: impl Fn(V3) -> bool,
    ) -> Option<ProbeSample> {
        let mut nearest: [(f32, Option<&ReverbProbe>); 4] = [(f32::INFINITY, None); 4];
        for probe in self.cells.values().flat_map(|c| &c.probes) {
            let d = distance(point, probe.position);
            if d > radius || d >= nearest[3].0 || !accept(probe.position) {
                continue;
            }
            let at = nearest.partition_point(|(n, _)| *n <= d);
            nearest.copy_within(at..3, at + 1);
            nearest[at] = (d, Some(probe));
        }
        let first = nearest[0].0;
        let mut weight = 0.0;
        let mut sample = ProbeSample {
            rt60: [0.0; 3],
            mean_free_path: 0.0,
            level: 0.0,
            nearest: first,
        };
        for (d, probe) in nearest {
            let Some(p) = probe else { continue };
            let w = 1.0 / (d * d).max(1e-4);
            weight += w;
            for b in 0..3 {
                sample.rt60[b] += p.rt60[b] * w;
            }
            sample.mean_free_path += p.mean_free_path * w;
            sample.level += p.level() * w;
        }
        if weight == 0.0 {
            return None;
        }
        sample.rt60 = sample.rt60.map(|t| t / weight);
        sample.mean_free_path /= weight;
        sample.level /= weight;
        Some(sample)
    }
}
