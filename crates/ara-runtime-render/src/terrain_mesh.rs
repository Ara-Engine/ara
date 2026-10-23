//! Terrain in presented (windowed) frames: each resident chunk's heightfield is drawn
//! by the scene renderer as a world-space mesh, so it takes shadows and every light
//! like any surface, coloured by its splat weights through a per-tile base-colour
//! texture (grass, rock, soil, snow). Captures composite ara-landscape (CDLOD terrain
//! and foliage) by depth instead, so the presenter adds these draws only to frames it
//! presents.
use crate::geometry::Source;
use ara_render::scene;
use ara_runtime::{model::ModelVertex, terrain::TerrainData};
use ara_terrain::{Tile, TileCoord};
use glam::Vec3;
use std::{collections::BTreeMap, sync::Arc};

/// Splat channel colours (sRGB): grass, rock, soil, snow.
const LAYERS: [[f32; 3]; 4] = [
    [0.24, 0.35, 0.13],
    [0.43, 0.41, 0.38],
    [0.37, 0.28, 0.18],
    [0.88, 0.89, 0.92],
];

/// One tile's world-space mesh and colour texture.
pub struct TileMesh {
    /// Render identity (terrain range, distinct from entity draws).
    pub entity: u64,
    /// Legend label.
    pub label: String,
    /// Content key (terrain data and coordinate), for the presenter's layout.
    pub key: u64,
    /// World-space vertices (identity transform) and triangles.
    pub source: Arc<Source>,
    /// Splat-coloured base colour, one texel per height sample.
    pub texture: Arc<scene::Texture>,
}

/// Tile meshes of the current terrain, built on first use.
#[derive(Default)]
pub struct TerrainMeshes {
    data: Option<Arc<TerrainData>>,
    tiles: BTreeMap<TileCoord, Arc<TileMesh>>,
}

impl TerrainMeshes {
    /// The mesh of `coord` in `data` (rebuilt when the terrain data changes), or none
    /// when the terrain has no such tile.
    pub fn tile(&mut self, data: &Arc<TerrainData>, coord: TileCoord) -> Option<Arc<TileMesh>> {
        if !self.data.as_ref().is_some_and(|d| Arc::ptr_eq(d, data)) {
            self.tiles.clear();
            self.data = Some(Arc::clone(data));
        }
        if let Some(mesh) = self.tiles.get(&coord) {
            return Some(Arc::clone(mesh));
        }
        let tile = data.tiles.get(&coord)?;
        // The data's address identifies its generation while it is alive (held here).
        let generation = Arc::as_ptr(data) as usize as u64;
        let mesh = Arc::new(build(data, coord, tile, generation));
        self.tiles.insert(coord, Arc::clone(&mesh));
        Some(mesh)
    }
}

fn build(data: &TerrainData, coord: TileCoord, tile: &Tile, generation: u64) -> TileMesh {
    let header = &data.header;
    let cells = header.cells as usize;
    let side = cells + 1;
    let spacing = header.spacing;
    let width = header.tile_width();
    let origin = [
        header.origin[0] + coord.x as f32 * width,
        header.origin[1] + coord.z as f32 * width,
    ];
    let mut vertices = Vec::with_capacity(side * side);
    for z in 0..side {
        for x in 0..side {
            let (wx, wz) = (
                origin[0] + x as f32 * spacing,
                origin[1] + z as f32 * spacing,
            );
            let y = tile.heights[z * side + x];
            // Central differences across tile borders keep seams smooth.
            let at = |dx: f32, dz: f32| data.height(wx + dx, wz + dz).unwrap_or(y);
            let normal = Vec3::new(
                at(-spacing, 0.0) - at(spacing, 0.0),
                2.0 * spacing,
                at(0.0, -spacing) - at(0.0, spacing),
            )
            .normalize_or(Vec3::Y);
            vertices.push(ModelVertex {
                position: [wx, y, wz],
                normal: normal.to_array(),
                uv: [x as f32 / cells as f32, z as f32 / cells as f32],
                tangent: [1.0, 0.0, 0.0, 1.0],
                ..Default::default()
            });
        }
    }
    let mut indices = Vec::with_capacity(cells * cells * 6);
    let s = side as u32;
    for z in 0..cells {
        for x in 0..cells {
            if tile.holes.get(z * side + x).is_some_and(|h| *h != 0) {
                continue;
            }
            let a = (z * side + x) as u32;
            // Upward winding, as ara-terrain's heightfield meshes.
            indices.extend([a, a + s, a + 1, a + 1, a + s, a + s + 1]);
        }
    }
    let mut rgba = Vec::with_capacity(side * side * 4);
    for (i, weights) in tile.weights.iter().enumerate() {
        let total = weights.iter().map(|w| f32::from(*w)).sum::<f32>();
        let weights = if total > 0.0 {
            weights.map(|w| f32::from(w) / total)
        } else {
            [1.0, 0.0, 0.0, 0.0]
        };
        // A little deterministic speckle so flat colour reads as ground.
        let speckle = ((i as u32).wrapping_mul(2_654_435_761) >> 24) as f32 / 255.0 * 0.06 - 0.03;
        for k in 0..3 {
            let c: f32 = LAYERS.iter().zip(weights).map(|(l, w)| l[k] * w).sum();
            rgba.push(((c + speckle).clamp(0.0, 1.0) * 255.0) as u8);
        }
        rgba.push(255);
    }
    let entity =
        0x7e45_0000_0000_0000 | (u64::from(coord.x as u16) << 16) | u64::from(coord.z as u16);
    TileMesh {
        entity,
        label: format!("Terrain {},{}", coord.x, coord.z),
        key: entity ^ generation.rotate_left(17),
        source: Arc::new(Source { vertices, indices }),
        texture: Arc::new(scene::Texture {
            size: [side as u32, side as u32],
            rgba,
            srgb: true,
        }),
    }
}
