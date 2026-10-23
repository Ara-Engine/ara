//! Navmesh streaming with world cells: per-cell bakes, resident-set assembly
//! off the tick thread, and terrain heightfield input.
use crate::{
    BakeArtifact, BakeInput, BakeStats, Baker, CellCoord, CellGrid, ClusterGraph, ClusterSummary,
    Geometry, NavError, NavMesh, Obstacle, OffMeshLink,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    thread::JoinHandle,
};

/// One cell's bake: its tiles (baked with halo geometry, so seams match the
/// neighbours' independent bakes) and its cluster summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellBake {
    /// Baked cell.
    pub cell: CellCoord,
    /// Tiles of this cell only.
    pub artifact: BakeArtifact,
    /// Portals and costs for hierarchical routing.
    pub summary: ClusterSummary,
}

/// Authored input for one cell bake.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CellSource {
    /// Collision triangles covering the cell plus the bake halo (extras are ignored).
    pub geometry: Geometry,
    /// Complete authored link set.
    pub links: Vec<OffMeshLink>,
    /// Complete obstacle set.
    pub obstacles: Vec<Obstacle>,
}

/// Bakes one cell from `source`. Pass the cell's previous bake to reuse unchanged
/// tiles; `sample_spacing` sets the cluster attach grid (metres). WORKER ONLY.
pub fn bake_cell(
    baker: &Baker,
    grid: &CellGrid,
    cell: CellCoord,
    source: CellSource,
    previous: Option<&BakeArtifact>,
    sample_spacing: f32,
) -> Result<CellBake, NavError> {
    let input = BakeInput {
        settings: grid.settings.clone(),
        geometry: source.geometry,
        links: source.links,
        obstacles: source.obstacles,
        tiles: grid.tiles(cell),
    };
    let artifact = baker.bake(&input, previous)?;
    let mesh = NavMesh::from_artifact(&artifact)?;
    let summary = ClusterSummary::build(grid, cell, &mesh, sample_spacing)?;
    Ok(CellBake {
        cell,
        artifact,
        summary,
    })
}

/// Merges cell bakes into one artifact (tiles are disjoint per cell).
pub fn merge_cells<'a>(
    grid: &CellGrid,
    cells: impl IntoIterator<Item = &'a CellBake>,
) -> BakeArtifact {
    let mut tiles = BTreeMap::new();
    let mut links: BTreeMap<u32, OffMeshLink> = BTreeMap::new();
    for bake in cells {
        tiles.extend(bake.artifact.tiles.iter().map(|(c, t)| (*c, t.clone())));
        links.extend(bake.artifact.links.iter().map(|l| (l.id, l.clone())));
    }
    BakeArtifact {
        settings: grid.settings.clone(),
        tiles,
        links: links.into_values().collect(),
        stats: BakeStats::default(),
    }
}

/// An in-flight resident-set assembly.
type Assembly = (BTreeSet<CellCoord>, JoinHandle<Result<NavMesh, NavError>>);

/// Resident-set manager for one agent profile. The host forwards cell
/// arrivals/removals (for example from `ara_world::Interest`), polls without
/// waiting, and stages ready meshes on `NavWorld`. Agents keep following their
/// world-space corridors across swaps and replan within the tick budget.
pub struct NavStreamer {
    grid: CellGrid,
    cells: BTreeMap<CellCoord, Arc<CellBake>>,
    graph: Arc<ClusterGraph>,
    resident: BTreeSet<CellCoord>,
    requested: BTreeSet<CellCoord>,
    job: Option<Assembly>,
}

impl NavStreamer {
    /// Indexes every known cell bake and builds the always-resident cluster graph.
    pub fn new(grid: CellGrid, bakes: Vec<CellBake>) -> Result<Self, NavError> {
        let cells: BTreeMap<_, _> = bakes.into_iter().map(|b| (b.cell, Arc::new(b))).collect();
        let graph = ClusterGraph::new(
            grid.clone(),
            cells
                .values()
                .map(|b| Arc::new(b.summary.clone()))
                .collect(),
        )?;
        Ok(Self {
            grid,
            cells,
            graph: Arc::new(graph),
            resident: BTreeSet::new(),
            requested: BTreeSet::new(),
            job: None,
        })
    }
    /// Cell grid shared with the bakes.
    pub fn grid(&self) -> &CellGrid {
        &self.grid
    }
    /// Cluster graph for `NavWorld::set_cluster_graph`.
    pub fn graph(&self) -> Arc<ClusterGraph> {
        self.graph.clone()
    }
    /// Cells in the most recently assembled mesh.
    pub fn resident(&self) -> &BTreeSet<CellCoord> {
        &self.resident
    }
    /// Requests a resident set; assembly starts on a worker thread when it differs
    /// from the current or in-flight set. Unknown cells are ignored.
    pub fn request(&mut self, cells: BTreeSet<CellCoord>) {
        let cells: BTreeSet<_> = cells
            .into_iter()
            .filter(|c| self.cells.contains_key(c))
            .collect();
        self.requested = cells;
        let in_flight = self.job.as_ref().map(|(set, _)| set);
        if in_flight.is_none_or(|set| set != &self.requested)
            && (in_flight.is_some() || self.requested != self.resident)
        {
            let bakes: Vec<_> = self
                .requested
                .iter()
                .map(|c| self.cells[c].clone())
                .collect();
            let grid = self.grid.clone();
            let handle = std::thread::Builder::new()
                .name("ara-nav-stream".into())
                .spawn(move || {
                    NavMesh::from_artifact(&merge_cells(&grid, bakes.iter().map(Arc::as_ref)))
                });
            match handle {
                Ok(handle) => self.job = Some((self.requested.clone(), handle)),
                Err(_) => self.job = None,
            }
        }
    }
    /// Adds one loaded cell to the requested set.
    pub fn cell_loaded(&mut self, cell: CellCoord) {
        let mut cells = self.requested.clone();
        cells.insert(cell);
        self.request(cells);
    }
    /// Removes one unloaded cell from the requested set.
    pub fn cell_unloaded(&mut self, cell: CellCoord) {
        let mut cells = self.requested.clone();
        cells.remove(&cell);
        self.request(cells);
    }
    /// Returns a finished assembly without waiting; stage it on `NavWorld`.
    pub fn poll(&mut self) -> Option<Result<NavMesh, NavError>> {
        if !self.job.as_ref().is_some_and(|(_, h)| h.is_finished()) {
            return None;
        }
        self.finish()
    }
    /// Blocks until the in-flight assembly completes (tests, replays, loading screens).
    pub fn wait(&mut self) -> Option<Result<NavMesh, NavError>> {
        self.finish()
    }
    fn finish(&mut self) -> Option<Result<NavMesh, NavError>> {
        let (cells, handle) = self.job.take()?;
        let result = handle
            .join()
            .unwrap_or_else(|_| Err(NavError::new("stream", "mesh assembly panicked")));
        if result.is_ok() {
            self.resident = cells;
        }
        Some(result)
    }
}

impl Geometry {
    /// Appends a row-major heightfield (rows along +Z, `cells + 1` samples per
    /// row, `spacing` metres apart from `origin`) as upward-wound triangles with
    /// `area`. `holes` is empty or has one entry per sample; a nonzero entry at a
    /// cell's minimum corner omits that cell (the `ara-terrain` tile convention).
    pub fn append_heightfield(
        &mut self,
        origin: [f32; 3],
        spacing: f32,
        cells: u32,
        heights: &[f32],
        holes: &[u8],
        area: u8,
    ) -> Result<(), NavError> {
        let side = cells as usize + 1;
        if heights.len() != side * side
            || (!holes.is_empty() && holes.len() != side * side)
            || !(spacing.is_finite() && spacing > 0.0)
            || area > 15
        {
            return Err(NavError::new(
                "invalidGeometry",
                "heightfield needs (cells+1)^2 heights and holes, positive spacing, area 0..15",
            ));
        }
        let base = self.vertices.len() as u32;
        self.vertices
            .extend(heights.iter().enumerate().map(|(i, h)| {
                [
                    origin[0] + (i % side) as f32 * spacing,
                    origin[1] + h,
                    origin[2] + (i / side) as f32 * spacing,
                ]
            }));
        let w = cells + 1;
        for z in 0..cells {
            for x in 0..cells {
                if holes.get((z * w + x) as usize).is_some_and(|h| *h != 0) {
                    continue;
                }
                let a = base + z * w + x;
                self.triangles
                    .extend_from_slice(&[[a, a + w, a + 1], [a + 1, a + w, a + w + 1]]);
                self.areas.extend_from_slice(&[area, area]);
            }
        }
        Ok(())
    }
}
