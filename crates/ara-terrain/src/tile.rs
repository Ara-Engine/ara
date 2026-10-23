use crate::{Header, invalid};
use ara_diag::Diagnostic;
use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// Integer X/Z address in a terrain's fixed grid.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Schema,
    Serialize,
    Deserialize,
)]
#[ara(name = "TerrainCoord", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TileCoord {
    /// Tile index along world X.
    #[serde(default)]
    pub x: i32,
    /// Tile index along world Z.
    #[serde(default)]
    pub z: i32,
}

/// Immutable evaluated tile. Samples include the duplicated north/east border.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    /// Cells per edge, a power of two in 16..256.
    pub cells: u32,
    /// Row-major world-space heights in metres.
    pub heights: Vec<f32>,
    /// Four splat channels per sample, with exact byte sum 255.
    pub weights: Vec<[u8; 4]>,
    /// One hole flag per sample; each cell uses its lower-left sample.
    pub holes: Vec<u8>,
}
impl Tile {
    /// Flat initial tile, grass in the first channel.
    pub fn flat(cells: u32) -> Result<Self, Diagnostic> {
        if !(16..=256).contains(&cells) || !cells.is_power_of_two() {
            return Err(invalid("invalid tile cells"));
        }
        let n = (cells as usize + 1).pow(2);
        Ok(Self {
            cells,
            heights: vec![0.; n],
            weights: vec![[255, 0, 0, 0]; n],
            holes: vec![0; n],
        })
    }
    /// Rejects malformed/unbounded binary payloads before publication.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if !(16..=256).contains(&self.cells) || !self.cells.is_power_of_two() {
            return Err(invalid("invalid tile cells"));
        }
        let n = (self.cells as usize + 1).pow(2);
        if self.heights.len() != n
            || self.weights.len() != n
            || self.holes.len() != n
            || self.heights.iter().any(|h| !h.is_finite() || h.abs() > 1e7)
            || self
                .weights
                .iter()
                .any(|w| w.iter().map(|v| u32::from(*v)).sum::<u32>() != 255)
            || self.holes.iter().any(|h| *h > 1)
        {
            return Err(invalid("invalid tile payload"));
        }
        Ok(())
    }
    /// Versioned little-endian binary; no alignment or native-layout assumptions.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        self.validate()?;
        let mut bytes = Vec::with_capacity(16 + self.heights.len() * 9);
        bytes.extend_from_slice(b"ARTH");
        for word in [1, self.cells, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        for ((h, w), hole) in self.heights.iter().zip(&self.weights).zip(&self.holes) {
            bytes.extend_from_slice(&h.to_le_bytes());
            bytes.extend_from_slice(w);
            bytes.push(*hole);
        }
        Ok(bytes)
    }
    /// Decodes only validated exact-length ARTH v1 tile payloads.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        if bytes.len() < 16 || &bytes[..8] != b"ARTH\x01\0\0\0" || bytes[12..16] != [0; 4] {
            return Err(invalid("unsupported terrain tile header"));
        }
        let cells = u32::from_le_bytes(
            bytes[8..12]
                .try_into()
                .map_err(|_| invalid("truncated tile"))?,
        );
        let mut tile = Self::flat(cells)?;
        if bytes.len() != 16 + tile.heights.len() * 9 {
            return Err(invalid("incorrect tile length"));
        }
        for (i, sample) in bytes[16..].as_chunks::<9>().0.iter().enumerate() {
            tile.heights[i] = f32::from_le_bytes(
                sample[..4]
                    .try_into()
                    .map_err(|_| invalid("truncated height"))?,
            );
            tile.weights[i].copy_from_slice(&sample[4..8]);
            tile.holes[i] = sample[8];
        }
        tile.validate()?;
        Ok(tile)
    }
    /// Heightfield collision seam. Native Jolt adapters must preserve holes and spacing.
    pub fn heightfield<'a>(&'a self, header: &Header, coord: TileCoord) -> Heightfield<'a> {
        Heightfield {
            origin: [
                header.origin[0] + coord.x as f32 * header.tile_width(),
                0.,
                header.origin[1] + coord.z as f32 * header.tile_width(),
            ],
            spacing: header.spacing,
            cells: self.cells,
            heights: &self.heights,
            holes: &self.holes,
        }
    }
}

/// Borrowed Jolt heightfield input; its owner must outlive synchronous shape creation.
pub struct Heightfield<'a> {
    /// World-space tile origin in metres.
    pub origin: [f32; 3],
    /// Uniform horizontal sample spacing.
    pub spacing: f32,
    /// Cells per edge (there are cells + 1 samples).
    pub cells: u32,
    /// Row-major heights, including shared edges.
    pub heights: &'a [f32],
    /// Hole cells omitted from both collision and navigation.
    pub holes: &'a [u8],
}

/// Navigation/GI/shadow mesh input; positive-Y winding and holes match collision.
pub struct SurfaceMesh {
    /// World-space vertices.
    pub vertices: Vec<[f32; 3]>,
    /// Triangle indices with upward winding.
    pub triangles: Vec<[u32; 3]>,
}
impl Heightfield<'_> {
    /// Worker-only triangle extraction, directly assignable to ara-nav Geometry.
    pub fn mesh(&self) -> SurfaceMesh {
        let side = self.cells as usize + 1;
        let vertices = self
            .heights
            .iter()
            .enumerate()
            .map(|(i, h)| {
                [
                    self.origin[0] + (i % side) as f32 * self.spacing,
                    *h,
                    self.origin[2] + (i / side) as f32 * self.spacing,
                ]
            })
            .collect();
        let mut triangles = Vec::new();
        for z in 0..self.cells {
            for x in 0..self.cells {
                let a = z * (self.cells + 1) + x;
                if self.holes[a as usize] == 0 {
                    triangles.extend_from_slice(&[
                        [a, a + self.cells + 1, a + 1],
                        [a + 1, a + self.cells + 1, a + self.cells + 2],
                    ]);
                }
            }
        }
        SurfaceMesh {
            vertices,
            triangles,
        }
    }
}
