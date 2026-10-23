//! Validated collision inputs, walkability settings and stable tile coordinates.
use crate::{NavError, finite};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Axis-aligned world-space bounds. Tile X/Z bounds are half-open.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    /// Inclusive minimum, in metres.
    pub min: [f32; 3],
    /// Maximum, in metres.
    pub max: [f32; 3],
}
impl Bounds {
    /// Whether two bounds touch (inclusive for conservative invalidation).
    pub fn intersects(self, other: Self) -> bool {
        (0..3).all(|i| self.min[i] <= other.max[i] && self.max[i] >= other.min[i])
    }
    pub(crate) fn valid(self) -> bool {
        finite(self.min) && finite(self.max) && (0..3).all(|i| self.min[i] < self.max[i])
    }
    pub(crate) fn expanded(self, margin: f32) -> Self {
        let mut result = self;
        for i in [0, 2] {
            result.min[i] -= margin;
            result.max[i] += margin;
        }
        result
    }
}

/// A collision triangle soup; winding must face upward for walkable floors.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    /// World-space vertices shared by indexed triangles.
    pub vertices: Vec<[f32; 3]>,
    /// Vertex indices per triangle.
    pub triangles: Vec<[u32; 3]>,
    /// One area ID per triangle: 0..15, or 255 for solid non-walkable geometry.
    pub areas: Vec<u8>,
}

/// Walkability dimensions; each profile produces a separate mesh.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Stable project-owned profile ID.
    pub id: u32,
    /// Horizontal collision radius in metres.
    pub radius: f32,
    /// Required free height in metres.
    pub height: f32,
    /// Maximum step height in metres.
    pub climb: f32,
    /// Steepest walkable slope in degrees (strictly below 90).
    pub slope: f32,
}
impl Default for AgentProfile {
    fn default() -> Self {
        Self {
            id: 0,
            radius: 0.3,
            height: 1.8,
            climb: 0.4,
            slope: 45.0,
        }
    }
}

/// Voxelization and tile grid settings; grid origin remains stable across edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BakeSettings {
    /// Fixed world-space grid origin, including the minimum voxel height.
    pub origin: [f32; 3],
    /// Maximum voxel height; changing this invalidates all tiles.
    pub max_y: f32,
    /// Horizontal voxel size in metres.
    pub cell_size: f32,
    /// Vertical voxel size in metres.
    pub cell_height: f32,
    /// Interior tile width in voxels (8..256).
    pub tile_size: u16,
    /// Maximum contour simplification error in voxels.
    pub max_error: f32,
    /// Profile whose radius/height affect walkability.
    pub agent: AgentProfile,
}
impl Default for BakeSettings {
    fn default() -> Self {
        Self {
            origin: [0.0, -4.0, 0.0],
            max_y: 16.0,
            cell_size: 0.2,
            cell_height: 0.1,
            tile_size: 64,
            max_error: 1.3,
            agent: AgentProfile::default(),
        }
    }
}
impl BakeSettings {
    /// Tile width in world-space metres.
    pub fn tile_width(&self) -> f32 {
        self.cell_size * f32::from(self.tile_size)
    }
    /// Core bounds for a tile (the baker adds a walkability halo).
    pub fn tile_bounds(&self, coord: TileCoord) -> Bounds {
        let width = self.tile_width();
        let x = self.origin[0] + coord.x as f32 * width;
        let z = self.origin[2] + coord.z as f32 * width;
        Bounds {
            min: [x, self.origin[1], z],
            max: [x + width, self.max_y, z + width],
        }
    }
    pub(crate) fn validate(&self) -> Result<(), NavError> {
        let a = self.agent;
        if !finite(self.origin)
            || self.origin.iter().any(|v| v.abs() > 1_000_000.0)
            || !self.max_y.is_finite()
            || self.max_y.abs() > 1_000_000.0
            || self.max_y <= self.origin[1]
            || !(0.01..=10.0).contains(&self.cell_size)
            || !(0.001..=5.0).contains(&self.cell_height)
            || !(8..=256).contains(&self.tile_size)
            || !(0.0..=10.0).contains(&self.max_error)
            || !(0.0..=10.0).contains(&a.radius)
            || !(0.01..=100.0).contains(&a.height)
            || !(0.0..=10.0).contains(&a.climb)
            || !(0.0..89.0).contains(&a.slope)
            || (self.max_y - self.origin[1]) / self.cell_height > 65534.0
            || a.height / self.cell_height > 65534.0
            || a.radius / self.cell_size > 256.0
        {
            return Err(NavError::new(
                "invalidSettings",
                "invalid voxel settings, bounds or agent dimensions",
            ));
        }
        Ok(())
    }
    pub(crate) fn halo(&self) -> f32 {
        ((self.agent.radius / self.cell_size).ceil() + 3.0) * self.cell_size
    }
}

/// Integer tile address, stable across geometry edits and stream operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TileCoord {
    /// X grid coordinate.
    pub x: i32,
    /// Z grid coordinate.
    pub z: i32,
}

/// An authored connection bridging a gap, ladder, jump or door transition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OffMeshLink {
    /// Stable nonzero ID used in diagnostics.
    pub id: u32,
    /// Starting point; its tile owns the connection.
    pub start: [f32; 3],
    /// Destination point.
    pub end: [f32; 3],
    /// Endpoint snapping radius in metres.
    pub radius: f32,
    /// Traversal area 0..15.
    pub area: u8,
    /// Whether agents may traverse both directions.
    pub bidirectional: bool,
    /// Disabled connections remain available to diagnostics but do not bake.
    pub enabled: bool,
}

/// A temporary cut-out; removing it restores the cached original tile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Obstacle {
    /// Stable ID; changes are canonically sorted by this ID.
    pub id: u64,
    /// Solid world-space box. The baker expands it by the profile radius/height.
    pub bounds: Bounds,
}

/// Complete geometry state plus the set of tiles to keep loaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BakeInput {
    /// Voxel and agent settings.
    pub settings: BakeSettings,
    /// Raw collision geometry; triangle order is canonicalized before baking.
    pub geometry: Geometry,
    /// Connections, sorted by ID by the baker.
    pub links: Vec<OffMeshLink>,
    /// Current temporary cut-outs, sorted by ID by the baker.
    pub obstacles: Vec<Obstacle>,
    /// Full desired tile set. Omitting a prior tile streams it out.
    pub tiles: BTreeSet<TileCoord>,
}
impl BakeInput {
    /// Selects all tiles intersecting bounds on a stable grid (including negative coordinates).
    pub fn tiles_in(
        settings: &BakeSettings,
        bounds: Bounds,
    ) -> Result<BTreeSet<TileCoord>, NavError> {
        settings.validate()?;
        if !bounds.valid() {
            return Err(NavError::new(
                "invalidBounds",
                "tile bounds must be finite and nonempty",
            ));
        }
        let w = settings.tile_width();
        let x0 = ((bounds.min[0] - settings.origin[0]) / w).floor();
        let z0 = ((bounds.min[2] - settings.origin[2]) / w).floor();
        let x1 = ((bounds.max[0] - settings.origin[0]) / w).ceil();
        let z1 = ((bounds.max[2] - settings.origin[2]) / w).ceil();
        if [x0, z0, x1, z1].iter().any(|x| x.abs() > 1_000_000.0) || (x1 - x0) * (z1 - z0) > 8192.0
        {
            return Err(NavError::new(
                "capacity",
                "tile range exceeds 8192 tiles or coordinate precision",
            ));
        }
        Ok((x0 as i32..x1 as i32)
            .flat_map(|x| (z0 as i32..z1 as i32).map(move |z| TileCoord { x, z }))
            .collect())
    }
    pub(crate) fn validate(&self) -> Result<(), NavError> {
        self.settings.validate()?;
        let g = &self.geometry;
        if g.vertices.len() > 6_000_000
            || g.triangles.len() > 2_000_000
            || self.links.len() > 65535
            || self.obstacles.len() > 65535
            || self.tiles.len() > 8192
            || g.areas.len() != g.triangles.len()
            || !g.vertices.iter().copied().all(finite)
            || g.vertices.iter().flatten().any(|v| v.abs() > 1_000_000.0)
            || g.triangles
                .iter()
                .flatten()
                .any(|&i| i as usize >= g.vertices.len())
            || g.areas.iter().any(|&a| a > 15 && a != 255)
            || self
                .tiles
                .iter()
                .any(|t| t.x.abs_diff(0) > 1_000_000 || t.z.abs_diff(0) > 1_000_000)
        {
            return Err(NavError::new(
                "invalidGeometry",
                "invalid triangle indices, areas, values or tile count",
            ));
        }
        if self.tiles.iter().any(|&coord| {
            let bounds = self.settings.tile_bounds(coord);
            !bounds.valid()
                || bounds
                    .min
                    .into_iter()
                    .chain(bounds.max)
                    .any(|v| v.abs() > 1_000_000.0)
        }) {
            return Err(NavError::new(
                "invalidBounds",
                "tile grid exceeds supported world coordinate range",
            ));
        }
        let mut ids = BTreeSet::new();
        for l in &self.links {
            if l.id == 0
                || !ids.insert(l.id)
                || !finite(l.start)
                || !finite(l.end)
                || l.start
                    .into_iter()
                    .chain(l.end)
                    .any(|v| v.abs() > 1_000_000.0)
                || !(0.001..=100.0).contains(&l.radius)
                || l.area > 15
            {
                return Err(NavError::new(
                    "invalidLink",
                    "links require unique nonzero IDs, finite endpoints and positive radii",
                ));
            }
            let width = self.settings.tile_width();
            let coord = |p: [f32; 3]| TileCoord {
                x: ((p[0] - self.settings.origin[0]) / width).floor() as i32,
                z: ((p[2] - self.settings.origin[2]) / width).floor() as i32,
            };
            let start = coord(l.start);
            let end = coord(l.end);
            if l.enabled
                && self.tiles.contains(&start)
                && (start.x.abs_diff(end.x) > 1 || start.z.abs_diff(end.z) > 1)
            {
                return Err(NavError::new(
                    "linkSpan",
                    "Detour links require adjacent endpoint tiles; increase tile_size for this link span",
                ));
            }
        }
        let mut ids = BTreeSet::new();
        for b in &self.obstacles {
            if !ids.insert(b.id) || !b.bounds.valid() {
                return Err(NavError::new(
                    "invalidObstacle",
                    "obstacles require unique IDs and finite nonempty bounds",
                ));
            }
        }
        Ok(())
    }
}
