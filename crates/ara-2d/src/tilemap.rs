//! Text source cells, deterministic auto-tiling, cache chunks and collision geometry.
use ara_schema::{AssetRef, Schema};
use ara_sim::bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Tile coordinate layout; hex is axial and pointy-topped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GridLayout {
    /// Axis-aligned rectangular cells.
    #[default]
    Grid,
    /// Axial q/r hexagons with cell size equal to their bounding box.
    Hex,
    /// Diamond grid, cell size equal to a diamond's bounding box.
    Isometric,
}
impl GridLayout {
    /// World-space centre offset for a signed cell coordinate.
    pub fn centre(self, x: i32, y: i32, size: [f32; 2]) -> [f32; 2] {
        let (x, y) = (x as f32, y as f32);
        match self {
            Self::Grid => [x * size[0], y * size[1]],
            Self::Hex => [(x + y * 0.5) * size[0], y * size[1] * 0.75],
            Self::Isometric => [(x - y) * size[0] * 0.5, (x + y) * size[1] * 0.5],
        }
    }
    /// Defined clockwise neighbor order used by rule masks.
    pub fn neighbors(self) -> &'static [(i32, i32)] {
        match self {
            Self::Hex => &[(1, 0), (0, 1), (-1, 1), (-1, 0), (0, -1), (1, -1)],
            _ => &[(1, 0), (0, 1), (-1, 0), (0, -1)],
        }
    }
}

/// One occupied source cell; tile zero is reserved for empty space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Tile {
    /// Tileset definition number, positive for occupied cells.
    pub tile: u32,
    /// Semantic terrain group used by auto-tiling.
    pub terrain: u32,
}
/// A tile's visual and collision definition.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TileDefinition {
    /// Stable atlas region name.
    pub region: String,
    /// Generate a static collider and shadow silhouette from this cell.
    pub solid: bool,
    /// Opt into local lights and normal maps.
    pub lit: bool,
}
/// One map layer with stable identity, source cells and independent painter order.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TileLayer {
    /// Stable layer identity, used in field paths and cache keys.
    pub id: String,
    /// Sorting layer shared with sprites.
    pub sorting_layer: i32,
    /// Draw order within the sorting layer.
    pub order: i32,
    /// Parallax factor per world axis.
    #[ara(default = [1, 1])]
    pub parallax: [f32; 2],
    /// Signed `x,y` keys, sorted canonically; missing cells are empty.
    pub cells: BTreeMap<String, Tile>,
}
impl Default for TileLayer {
    fn default() -> Self {
        Self {
            id: String::new(),
            sorting_layer: 0,
            order: 0,
            parallax: [1.; 2],
            cells: BTreeMap::new(),
        }
    }
}
/// First matching rule wins; masks follow GridLayout::neighbors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TileRule {
    /// Terrain group to transform.
    pub terrain: u32,
    /// Neighbors which must share this terrain.
    pub required: u32,
    /// Neighbors which must not share this terrain.
    pub forbidden: u32,
    /// Output tile number, without changing semantic terrain.
    pub tile: u32,
}
/// Canonical text source for a tilemap. Binary chunks are derived cache products.
#[derive(Debug, Clone, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "twoD")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Tilemap {
    /// Atlas asset reference; never a native descriptor.
    pub atlas: Option<AssetRef>,
    /// Optional normal atlas matching the source regions.
    pub normal_atlas: Option<AssetRef>,
    /// Grid, pointy hex or diamond layout.
    pub layout: GridLayout,
    /// World dimensions of a cell's bounding box in metres.
    #[ara(default = [1, 1])]
    pub cell_size: [f32; 2],
    /// Tile number text to definitions; zero is reserved.
    pub definitions: BTreeMap<String, TileDefinition>,
    /// Stable layers. Their array order never determines painter order.
    #[ara(keyed)]
    pub layers: Vec<TileLayer>,
    /// Ordered terrain rules.
    pub rules: Vec<TileRule>,
}
impl Default for Tilemap {
    fn default() -> Self {
        Self {
            atlas: None,
            normal_atlas: None,
            layout: GridLayout::Grid,
            cell_size: [1.; 2],
            definitions: BTreeMap::new(),
            layers: Vec::new(),
            rules: Vec::new(),
        }
    }
}

/// Canonical source coordinate spelling (no leading zeros or whitespace).
pub fn key(x: i32, y: i32) -> String {
    format!("{x},{y}")
}
/// Parses and validates a source coordinate without accepting aliases.
pub fn coordinate(s: &str) -> anyhow::Result<(i32, i32)> {
    let (x, y) = s
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("expected x,y coordinate"))?;
    let pair = (x.parse::<i32>()?, y.parse::<i32>()?);
    anyhow::ensure!(key(pair.0, pair.1) == s, "noncanonical tile coordinate");
    Ok(pair)
}
impl Tilemap {
    /// Checks source limits, stable identities and all referenced definitions.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.cell_size.iter().all(|v| v.is_finite() && *v > 0.),
            "invalid cell size"
        );
        let mut ids = BTreeSet::new();
        for layer in &self.layers {
            anyhow::ensure!(
                ara_schema::grammar::split_id(&layer.id).is_some() && ids.insert(&layer.id),
                "duplicate/invalid layer ID; use l_ground style identities"
            );
            anyhow::ensure!(
                layer.parallax.iter().all(|v| v.is_finite()),
                "nonfinite parallax"
            );
            for (k, t) in &layer.cells {
                coordinate(k)?;
                anyhow::ensure!(
                    t.tile > 0 && self.definitions.contains_key(&t.tile.to_string()),
                    "unknown/empty tile definition"
                );
            }
        }
        let mask = (1 << self.layout.neighbors().len()) - 1;
        for rule in &self.rules {
            anyhow::ensure!(
                rule.required & rule.forbidden == 0
                    && (rule.required | rule.forbidden) & !mask == 0
                    && rule.tile > 0
                    && self.definitions.contains_key(&rule.tile.to_string()),
                "invalid tile rule"
            );
        }
        Ok(())
    }
}

/// Derived 32x32 cache chunk. Bytes are LE and independent of host struct layouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Stable layer identity.
    pub layer: String,
    /// Signed chunk coordinates (Euclidean division).
    pub coordinate: [i32; 2],
    /// BLAKE3 of versioned bytes; use as the cache object identity.
    pub digest: String,
    /// Header plus 1024 pairs of tile and terrain numbers, row-major.
    pub bytes: Vec<u8>,
}
/// Builds immutable cache chunks from canonical source; run outside editor frames.
pub fn chunks(map: &Tilemap) -> anyhow::Result<Vec<Chunk>> {
    map.validate()?;
    let mut result = Vec::new();
    for layer in &map.layers {
        let mut groups = BTreeMap::<(i32, i32), Vec<Tile>>::new();
        for (k, tile) in &layer.cells {
            let (x, y) = coordinate(k)?;
            let cells = groups
                .entry((x.div_euclid(32), y.div_euclid(32)))
                .or_insert_with(|| vec![Tile::default(); 1024]);
            cells[(y.rem_euclid(32) * 32 + x.rem_euclid(32)) as usize] = *tile;
        }
        for ((x, y), tiles) in groups {
            let mut bytes = b"AR2C\x01\0\0\0".to_vec();
            bytes.extend(x.to_le_bytes());
            bytes.extend(y.to_le_bytes());
            for tile in tiles {
                bytes.extend(tile.tile.to_le_bytes());
                bytes.extend(tile.terrain.to_le_bytes());
            }
            result.push(Chunk {
                layer: layer.id.clone(),
                coordinate: [x, y],
                digest: blake3::hash(&bytes).to_hex().to_string(),
                bytes,
            });
        }
    }
    result.sort_by(|a, b| (&a.layer, a.coordinate).cmp(&(&b.layer, b.coordinate)));
    Ok(result)
}

/// Computes all rule changes from one immutable source, so traversal cannot cascade.
pub fn apply_rules(map: &Tilemap, layer: &TileLayer) -> anyhow::Result<Vec<(String, Tile)>> {
    let mut changes = Vec::new();
    for (key, cell) in &layer.cells {
        let (x, y) = coordinate(key)?;
        let mut mask = 0;
        for (bit, &(dx, dy)) in map.layout.neighbors().iter().enumerate() {
            if let (Some(nx), Some(ny)) = (x.checked_add(dx), y.checked_add(dy))
                && layer
                    .cells
                    .get(&self::key(nx, ny))
                    .is_some_and(|n| n.terrain == cell.terrain)
            {
                mask |= 1 << bit;
            }
        }
        if let Some(rule) = map.rules.iter().find(|r| {
            r.terrain == cell.terrain && mask & r.required == r.required && mask & r.forbidden == 0
        }) && rule.tile != cell.tile
        {
            changes.push((
                key.clone(),
                Tile {
                    tile: rule.tile,
                    ..*cell
                },
            ));
        }
    }
    Ok(changes)
}

/// Convex world-space tile polygon, suitable for static physics and shadow edges.
#[derive(Debug, Clone, PartialEq)]
pub struct TileCollider {
    /// Source layer.
    pub layer: String,
    /// Counter-clockwise world-space vertices.
    pub vertices: Vec<[f32; 2]>,
}
/// Merges grid rectangles greedily; hex and iso preserve their convex silhouettes.
pub fn colliders(map: &Tilemap) -> anyhow::Result<Vec<TileCollider>> {
    map.validate()?;
    let mut output = Vec::new();
    for layer in &map.layers {
        let mut solid = BTreeSet::new();
        for (k, t) in &layer.cells {
            if map.definitions[&t.tile.to_string()].solid {
                solid.insert(coordinate(k)?);
            }
        }
        while let Some(&(x, y)) = solid.first() {
            let mut w = 1i32;
            let mut h = 1i32;
            if map.layout == GridLayout::Grid {
                while x.checked_add(w).is_some_and(|nx| solid.contains(&(nx, y))) {
                    w += 1;
                }
                while let Some(ny) = y.checked_add(h) {
                    if !(0..w).all(|dx| solid.contains(&(x + dx, ny))) {
                        break;
                    }
                    h += 1;
                }
            }
            for dy in 0..h {
                for dx in 0..w {
                    solid.remove(&(x + dx, y + dy));
                }
            }
            let [cx, cy] = map.layout.centre(x, y, map.cell_size);
            let [sx, sy] = map.cell_size.map(|v| v * 0.5);
            let points = match map.layout {
                GridLayout::Grid => vec![
                    [cx - sx, cy - sy],
                    [cx + sx * (w * 2 - 1) as f32, cy - sy],
                    [cx + sx * (w * 2 - 1) as f32, cy + sy * (h * 2 - 1) as f32],
                    [cx - sx, cy + sy * (h * 2 - 1) as f32],
                ],
                GridLayout::Isometric => {
                    vec![[cx, cy - sy], [cx + sx, cy], [cx, cy + sy], [cx - sx, cy]]
                }
                GridLayout::Hex => vec![
                    [cx, cy - sy],
                    [cx + sx, cy - sy * 0.5],
                    [cx + sx, cy + sy * 0.5],
                    [cx, cy + sy],
                    [cx - sx, cy + sy * 0.5],
                    [cx - sx, cy - sy * 0.5],
                ],
            };
            output.push(TileCollider {
                layer: layer.id.clone(),
                vertices: points,
            });
        }
    }
    Ok(output)
}
