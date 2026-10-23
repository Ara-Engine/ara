//! Text source cells, deterministic auto-tiling, cache chunks and collision geometry.
//! A cell is one tile number; its terrain, collision and lighting come from the tile's
//! definition, so map text stays one short line per occupied cell.
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
    /// Rule-mask neighbor order, counter-clockwise from +X (+Y up). Grid and isometric
    /// use eight neighbors (E, NE, N, NW, W, SW, S, SE); hex uses its six axial neighbors.
    pub fn neighbors(self) -> &'static [(i32, i32)] {
        match self {
            Self::Hex => &[(1, 0), (0, 1), (-1, 1), (-1, 0), (0, -1), (1, -1)],
            _ => &[
                (1, 0),
                (1, 1),
                (0, 1),
                (-1, 1),
                (-1, 0),
                (-1, -1),
                (0, -1),
                (1, -1),
            ],
        }
    }
    /// Edge-adjacent neighbors, used by flood fill (grid/iso: four; hex: six).
    pub fn adjacent(self) -> &'static [(i32, i32)] {
        match self {
            Self::Hex => self.neighbors(),
            _ => &[(1, 0), (0, 1), (-1, 0), (0, -1)],
        }
    }
}

/// A tile's visual, terrain, collision and lighting definition.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TileDefinition {
    /// Stable atlas region name.
    pub region: String,
    /// Auto-tiling terrain group; zero opts out of rules.
    pub terrain: u32,
    /// Generate a static collider and shadow silhouette from this cell.
    pub solid: bool,
    /// Opt into local lights and normal maps.
    pub lit: bool,
}
/// One map layer with stable identity, source cells and independent painter order.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TileLayer {
    /// Stable layer identity (`l_ground`), used in field paths and cache keys.
    pub id: String,
    /// Sorting layer shared with sprites.
    pub sorting_layer: i32,
    /// Draw order within the sorting layer.
    pub order: i32,
    /// Parallax factor per world axis.
    #[ara(default = [1, 1])]
    pub parallax: [f32; 2],
    /// Linear, straight-alpha tint applied to every tile, e.g. to darken backgrounds.
    #[ara(default = [1, 1, 1, 1])]
    pub tint: [f32; 4],
    /// Signed `x,y` keys to positive tile numbers; missing cells are empty.
    pub cells: BTreeMap<String, u32>,
}
impl Default for TileLayer {
    fn default() -> Self {
        Self {
            id: String::new(),
            sorting_layer: 0,
            order: 0,
            parallax: [1.; 2],
            tint: [1.; 4],
            cells: BTreeMap::new(),
        }
    }
}
/// First matching rule wins. Masks use `GridLayout::neighbors` bit order; a neighbor
/// "matches" when its tile's definition has the rule's terrain.
#[derive(Debug, Clone, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TileRule {
    /// Terrain group to transform.
    pub terrain: u32,
    /// Neighbors which must share this terrain.
    pub required: u32,
    /// Neighbors which must not share this terrain.
    pub forbidden: u32,
    /// Output tile number; its definition must have the same terrain.
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
    /// Tile number text to definitions; zero is reserved for empty cells.
    pub definitions: BTreeMap<String, TileDefinition>,
    /// Stable layers. Their array order never determines painter order.
    #[ara(keyed)]
    pub layers: Vec<TileLayer>,
    /// Ordered auto-tiling rules.
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
    /// A defined tile's definition; zero and unknown numbers are None.
    pub fn definition(&self, tile: u32) -> Option<&TileDefinition> {
        self.definitions.get(&tile.to_string())
    }
    fn terrain(&self, tile: Option<&u32>) -> u32 {
        tile.and_then(|t| self.definition(*t))
            .map_or(0, |d| d.terrain)
    }
    /// Checks source limits, stable identities and all referenced definitions.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.cell_size.iter().all(|v| v.is_finite() && *v > 0.),
            "invalid cell size"
        );
        for k in self.definitions.keys() {
            anyhow::ensure!(
                k.parse::<u32>().is_ok_and(|n| n > 0 && n.to_string() == *k),
                "tile definition keys are canonical positive numbers"
            );
        }
        let mut ids = BTreeSet::new();
        for layer in &self.layers {
            anyhow::ensure!(
                ara_schema::grammar::split_id(&layer.id).is_some() && ids.insert(&layer.id),
                "duplicate/invalid layer ID; use l_ground style identities"
            );
            anyhow::ensure!(
                layer
                    .parallax
                    .iter()
                    .chain(&layer.tint)
                    .all(|v| v.is_finite()),
                "nonfinite parallax/tint"
            );
            for (k, t) in &layer.cells {
                coordinate(k)?;
                anyhow::ensure!(
                    self.definition(*t).is_some(),
                    "cell {k} uses unknown tile {t}"
                );
            }
        }
        let mask = (1u32 << self.layout.neighbors().len()) - 1;
        for rule in &self.rules {
            anyhow::ensure!(
                rule.terrain > 0
                    && rule.required & rule.forbidden == 0
                    && (rule.required | rule.forbidden) & !mask == 0
                    && self
                        .definition(rule.tile)
                        .is_some_and(|d| d.terrain == rule.terrain),
                "invalid tile rule: masks overlap/overflow or output tile has another terrain"
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
    /// `AR2C` v2 header, chunk coordinate, then 1024 row-major u32 tile numbers.
    pub bytes: Vec<u8>,
}
/// Builds immutable cache chunks from canonical source; run outside editor frames.
pub fn chunks(map: &Tilemap) -> anyhow::Result<Vec<Chunk>> {
    map.validate()?;
    let mut result = Vec::new();
    for layer in &map.layers {
        let mut groups = BTreeMap::<(i32, i32), Vec<u32>>::new();
        for (k, tile) in &layer.cells {
            let (x, y) = coordinate(k)?;
            let cells = groups
                .entry((x.div_euclid(32), y.div_euclid(32)))
                .or_insert_with(|| vec![0; 1024]);
            cells[(y.rem_euclid(32) * 32 + x.rem_euclid(32)) as usize] = *tile;
        }
        for ((x, y), tiles) in groups {
            let mut bytes = b"AR2C\x02\0\0\0".to_vec();
            bytes.extend(x.to_le_bytes());
            bytes.extend(y.to_le_bytes());
            for tile in tiles {
                bytes.extend(tile.to_le_bytes());
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

/// Computes rule changes from one immutable layer snapshot, so traversal order cannot
/// cascade. `only` restricts outputs to the given cells (e.g. a stroke and its border).
pub fn apply_rules(
    map: &Tilemap,
    layer: &TileLayer,
    only: Option<&BTreeSet<(i32, i32)>>,
) -> anyhow::Result<Vec<(String, u32)>> {
    let mut changes = Vec::new();
    for (key, tile) in &layer.cells {
        let (x, y) = coordinate(key)?;
        let terrain = map.terrain(Some(tile));
        if terrain == 0 || only.is_some_and(|cells| !cells.contains(&(x, y))) {
            continue;
        }
        let mut mask = 0;
        for (bit, &(dx, dy)) in map.layout.neighbors().iter().enumerate() {
            if let (Some(nx), Some(ny)) = (x.checked_add(dx), y.checked_add(dy))
                && map.terrain(layer.cells.get(&self::key(nx, ny))) == terrain
            {
                mask |= 1 << bit;
            }
        }
        if let Some(rule) = map.rules.iter().find(|r| {
            r.terrain == terrain && mask & r.required == r.required && mask & r.forbidden == 0
        }) && rule.tile != *tile
        {
            changes.push((key.clone(), rule.tile));
        }
    }
    Ok(changes)
}

/// Convex world-space tile polygon, suitable for static physics and shadow edges.
#[derive(Debug, Clone, PartialEq)]
pub struct TileCollider {
    /// Source layer.
    pub layer: String,
    /// Counter-clockwise map-space vertices.
    pub vertices: Vec<[f32; 2]>,
}
/// Merges grid rectangles greedily; hex and iso preserve their convex silhouettes.
pub fn colliders(map: &Tilemap) -> anyhow::Result<Vec<TileCollider>> {
    map.validate()?;
    let mut output = Vec::new();
    for layer in &map.layers {
        let mut solid = BTreeSet::new();
        for (k, t) in &layer.cells {
            if map.definition(*t).is_some_and(|d| d.solid) {
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

/// Shadow silhouette: map-space segments `[ax, ay, bx, by]` on the boundary of each layer's
/// solid cells, wound counter-clockwise (solid on the left). Interior seams never appear,
/// so a fragment inside solid ground is not shadowed by its own neighbors. Grid edges are
/// merged into maximal runs.
pub fn outline(map: &Tilemap) -> anyhow::Result<Vec<[f32; 4]>> {
    map.validate()?;
    let [w, h] = map.cell_size;
    let mut output = Vec::new();
    for layer in &map.layers {
        let mut solid = BTreeSet::new();
        for (k, t) in &layer.cells {
            if map.definition(*t).is_some_and(|d| d.solid) {
                solid.insert(coordinate(k)?);
            }
        }
        let open = |x: i32, dx: i32, y: i32, dy: i32| match (x.checked_add(dx), y.checked_add(dy)) {
            (Some(nx), Some(ny)) => !solid.contains(&(nx, ny)),
            _ => true,
        };
        if map.layout == GridLayout::Grid {
            // (side, line, start) unit edges; side 0..4 = bottom, right, top, left.
            let mut edges = BTreeSet::new();
            for &(x, y) in &solid {
                for (side, (dx, dy)) in [(0, -1), (1, 0), (0, 1), (-1, 0)].into_iter().enumerate() {
                    if open(x, dx, y, dy) {
                        let (line, start) = if side % 2 == 0 { (y, x) } else { (x, y) };
                        edges.insert((side, line, start));
                    }
                }
            }
            let mut runs: Vec<(usize, i32, i32, i32)> = Vec::new();
            for (side, line, start) in edges {
                match runs.last_mut() {
                    Some(run) if run.0 == side && run.1 == line && run.3 + 1 == start => {
                        run.3 = start
                    }
                    _ => runs.push((side, line, start, start)),
                }
            }
            for (side, line, first, last) in runs {
                let (line, first, last) = (line as f32, first as f32 - 0.5, last as f32 + 0.5);
                output.push(match side {
                    0 => [first * w, (line - 0.5) * h, last * w, (line - 0.5) * h],
                    1 => [(line + 0.5) * w, first * h, (line + 0.5) * w, last * h],
                    2 => [last * w, (line + 0.5) * h, first * w, (line + 0.5) * h],
                    _ => [(line - 0.5) * w, last * h, (line - 0.5) * w, first * h],
                });
            }
        } else {
            // Counter-clockwise polygon sides and the neighbor across each.
            let (sx, sy) = (w * 0.5, h * 0.5);
            for &(x, y) in &solid {
                let [cx, cy] = map.layout.centre(x, y, map.cell_size);
                let (corners, across): (Vec<[f32; 2]>, &[(i32, i32)]) = match map.layout {
                    GridLayout::Isometric => (
                        vec![[cx, cy - sy], [cx + sx, cy], [cx, cy + sy], [cx - sx, cy]],
                        &[(0, -1), (1, 0), (0, 1), (-1, 0)],
                    ),
                    _ => (
                        vec![
                            [cx, cy - sy],
                            [cx + sx, cy - sy * 0.5],
                            [cx + sx, cy + sy * 0.5],
                            [cx, cy + sy],
                            [cx - sx, cy + sy * 0.5],
                            [cx - sx, cy - sy * 0.5],
                        ],
                        &[(1, -1), (1, 0), (0, 1), (-1, 1), (-1, 0), (0, -1)],
                    ),
                };
                for (i, &(dx, dy)) in across.iter().enumerate() {
                    if open(x, dx, y, dy) {
                        let [a, b] = [corners[i], corners[(i + 1) % corners.len()]];
                        output.push([a[0], a[1], b[0], b[1]]);
                    }
                }
            }
        }
    }
    Ok(output)
}
