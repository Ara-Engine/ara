//! Hierarchical pathfinding: each streaming cell is a cluster with border portals.
//!
//! A cell is baked alone (its tiles plus halo geometry), then summarised: walkable
//! border intervals become portals, a Detour Dijkstra from every portal gives
//! portal-to-portal costs, and a coarse sample grid attaches start/goal points
//! without the cell's mesh being resident. Summaries of every cell form one
//! [`ClusterGraph`] (small, always resident); A* over it returns a [`Route`] of
//! portal crossings that agents follow leg by leg on the resident mesh.
use crate::{BakeSettings, Bounds, NavError, NavMesh, QueryFilter, Route, TileCoord, distance};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    sync::Arc,
};

/// Unreachable cost marker in summaries (JSON-safe, unlike infinity).
pub const UNREACHABLE: f32 = -1.0;

/// Streaming cell (cluster) address; equals `ara-world` `CellKey` x/z when the
/// world cell size matches [`CellGrid::cell_width`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CellCoord {
    /// Grid X.
    pub x: i32,
    /// Grid Z.
    pub z: i32,
}

/// Cells laid over a bake tile grid: each cell spans `tiles_per_cell` tiles per axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellGrid {
    /// Shared bake settings (origin, voxels, agent profile).
    pub settings: BakeSettings,
    /// Tiles per cell edge (1..=64).
    pub tiles_per_cell: u16,
}

impl CellGrid {
    /// Validates the settings and cell span.
    pub fn new(settings: BakeSettings, tiles_per_cell: u16) -> Result<Self, NavError> {
        settings.validate()?;
        if !(1..=64).contains(&tiles_per_cell) {
            return Err(NavError::new(
                "invalidGrid",
                "tiles per cell must be 1..=64",
            ));
        }
        Ok(Self {
            settings,
            tiles_per_cell,
        })
    }
    /// Cell edge length in metres.
    pub fn cell_width(&self) -> f32 {
        self.settings.tile_width() * f32::from(self.tiles_per_cell)
    }
    /// Cell containing a point (half-open in X/Z).
    pub fn cell_of(&self, p: [f32; 3]) -> CellCoord {
        let w = self.cell_width();
        CellCoord {
            x: ((p[0] - self.settings.origin[0]) / w).floor() as i32,
            z: ((p[2] - self.settings.origin[2]) / w).floor() as i32,
        }
    }
    /// World bounds of a cell (full bake height).
    pub fn bounds(&self, cell: CellCoord) -> Bounds {
        let w = self.cell_width();
        let x = self.settings.origin[0] + cell.x as f32 * w;
        let z = self.settings.origin[2] + cell.z as f32 * w;
        Bounds {
            min: [x, self.settings.origin[1], z],
            max: [x + w, self.settings.max_y, z + w],
        }
    }
    /// Tiles belonging to a cell.
    pub fn tiles(&self, cell: CellCoord) -> BTreeSet<TileCoord> {
        let n = i32::from(self.tiles_per_cell);
        (cell.x * n..cell.x * n + n)
            .flat_map(|x| (cell.z * n..cell.z * n + n).map(move |z| TileCoord { x, z }))
            .collect()
    }
    /// Cells intersecting bounds.
    pub fn cells_in(&self, bounds: Bounds) -> BTreeSet<CellCoord> {
        let a = self.cell_of(bounds.min);
        let b = self.cell_of(bounds.max);
        (a.x..=b.x)
            .flat_map(|x| (a.z..=b.z).map(move |z| CellCoord { x, z }))
            .collect()
    }
}

/// Cell side a portal lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Side {
    /// Maximum X border.
    PosX,
    /// Maximum Z border.
    PosZ,
    /// Minimum X border.
    NegX,
    /// Minimum Z border.
    NegZ,
}

/// A walkable interval on one cell border; agents cross clusters through it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Portal {
    /// Border the interval lies on.
    pub side: Side,
    /// Crossing point: interval midpoint on the border line.
    pub position: [f32; 3],
    /// Interval start along the border axis (Z for X sides, X for Z sides).
    pub from: f32,
    /// Interval end along the border axis.
    pub to: f32,
}

/// One cluster's precomputed connectivity. Costs are metres along polygon edge
/// midpoints (never below the straight-line distance), with base area costs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClusterSummary {
    /// Summarised cell.
    pub cell: CellCoord,
    /// Border portals, by side then interval.
    pub portals: Vec<Portal>,
    /// Row-major portal x portal costs; [`UNREACHABLE`] when disconnected inside the cell.
    pub costs: Vec<f32>,
    /// Attach samples per axis.
    pub samples_per_axis: u16,
    /// Sample points projected onto the cell mesh (row-major, Z rows); None off-mesh.
    pub samples: Vec<Option<[f32; 3]>>,
    /// Row-major sample x portal costs.
    pub sample_costs: Vec<f32>,
}

/// Longest portal interval before it is split (metres).
pub const MAX_PORTAL: f32 = 32.0;

impl ClusterSummary {
    /// Summarises `cell` from a mesh holding only that cell's tiles (baked with
    /// halos). Samples are spaced about `sample_spacing` metres apart. WORKER ONLY.
    pub fn build(
        grid: &CellGrid,
        cell: CellCoord,
        mesh: &NavMesh,
        sample_spacing: f32,
    ) -> Result<Self, NavError> {
        let n = i32::from(grid.tiles_per_cell);
        let (tx0, tz0) = (cell.x * n, cell.z * n);
        let (tx1, tz1) = (tx0 + n - 1, tz0 + n - 1);
        let bounds = grid.bounds(cell);
        let mut edges: BTreeMap<Side, Vec<(f32, f32, f32)>> = BTreeMap::new();
        for e in mesh.borders() {
            if e.linked != 0 {
                continue;
            }
            let side = match e.side {
                0 if e.tile_x == tx1 => Side::PosX,
                4 if e.tile_x == tx0 => Side::NegX,
                2 if e.tile_z == tz1 => Side::PosZ,
                6 if e.tile_z == tz0 => Side::NegZ,
                _ => continue,
            };
            let axis = if matches!(side, Side::PosX | Side::NegX) {
                2
            } else {
                0
            };
            let (a, b) = (e.a[axis].min(e.b[axis]), e.a[axis].max(e.b[axis]));
            if b - a > 1e-4 {
                edges
                    .entry(side)
                    .or_default()
                    .push((a, b, (e.a[1] + e.b[1]) * 0.5));
            }
        }
        let climb = grid.settings.agent.climb.max(0.25) + 0.5;
        let mut portals = Vec::new();
        for (side, mut list) in edges {
            list.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.total_cmp(&q.1)));
            let mut merged: Vec<(f32, f32, f32)> = Vec::new();
            for (a, b, y) in list {
                match merged.last_mut() {
                    Some(last) if a <= last.1 + 0.05 && (y - last.2).abs() <= climb => {
                        last.1 = last.1.max(b);
                    }
                    _ => merged.push((a, b, y)),
                }
            }
            for (a, b, y) in merged {
                let pieces = ((b - a) / MAX_PORTAL).ceil().max(1.0) as usize;
                let step = (b - a) / pieces as f32;
                for k in 0..pieces {
                    let (from, to) = (a + step * k as f32, a + step * (k + 1) as f32);
                    let mid = (from + to) * 0.5;
                    let position = match side {
                        Side::PosX => [bounds.max[0], y, mid],
                        Side::NegX => [bounds.min[0], y, mid],
                        Side::PosZ => [mid, y, bounds.max[2]],
                        Side::NegZ => [mid, y, bounds.min[2]],
                    };
                    portals.push(Portal {
                        side,
                        position,
                        from,
                        to,
                    });
                }
            }
        }

        let filter = QueryFilter::default();
        let mut query = mesh.query()?;
        let radius = grid.cell_width() * 1.5;
        let mut fields = Vec::with_capacity(portals.len());
        let mut portal_polys = Vec::with_capacity(portals.len());
        for p in &portals {
            let mut field = match query.dijkstra(p.position, &filter, radius)? {
                Some((field, false)) => field,
                Some((_, true)) => {
                    return Err(NavError::new(
                        "capacity",
                        "cluster Dijkstra exhausted its node pool; use smaller cells",
                    ));
                }
                None => Vec::new(),
            };
            field.sort_unstable_by_key(|(r, _)| *r);
            fields.push(field);
            portal_polys.push(query.nearest(p.position, &filter)?.map(|n| n.polygon));
        }
        let lookup = |field: &[(u32, f32)], poly: Option<u32>| {
            let poly = poly?;
            field
                .binary_search_by_key(&poly, |(r, _)| *r)
                .ok()
                .map(|i| field[i].1)
        };
        let count = portals.len();
        let mut costs = vec![UNREACHABLE; count * count];
        for i in 0..count {
            for j in 0..count {
                costs[i * count + j] = if i == j {
                    0.0
                } else {
                    lookup(&fields[i], portal_polys[j]).map_or(UNREACHABLE, |c| {
                        c.max(distance(portals[i].position, portals[j].position))
                    })
                };
            }
        }

        let w = grid.cell_width();
        let per_axis = ((w / sample_spacing.max(0.5)).ceil() as u16).clamp(1, 64);
        let step = w / f32::from(per_axis);
        let half_height = (grid.settings.max_y - grid.settings.origin[1]) * 0.5 + 1.0;
        query.set_extent([step * 0.5, half_height, step * 0.5])?;
        let mid_y = (grid.settings.max_y + grid.settings.origin[1]) * 0.5;
        let mut samples = Vec::new();
        let mut sample_costs = Vec::new();
        for sz in 0..per_axis {
            for sx in 0..per_axis {
                let point = [
                    bounds.min[0] + (f32::from(sx) + 0.5) * step,
                    mid_y,
                    bounds.min[2] + (f32::from(sz) + 0.5) * step,
                ];
                let near = query.nearest(point, &filter)?;
                samples.push(near.map(|n| n.position));
                for (j, field) in fields.iter().enumerate() {
                    sample_costs.push(
                        near.and_then(|n| lookup(field, Some(n.polygon))).map_or(
                            UNREACHABLE,
                            |c| {
                                c.max(
                                    near.map_or(0.0, |n| distance(n.position, portals[j].position)),
                                )
                            },
                        ),
                    );
                }
            }
        }
        Ok(Self {
            cell,
            portals,
            costs,
            samples_per_axis: per_axis,
            samples,
            sample_costs,
        })
    }

    /// Nearest projected sample to a point and its distance.
    fn attach(&self, p: [f32; 3]) -> Option<(usize, f32)> {
        self.samples
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.map(|s| (i, distance(s, p))))
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
    }
}

/// All cluster summaries as one portal graph (CSR adjacency, always resident).
pub struct ClusterGraph {
    grid: CellGrid,
    cells: BTreeMap<CellCoord, (u32, Arc<ClusterSummary>)>,
    positions: Vec<[f32; 3]>,
    offsets: Vec<u32>,
    edges: Vec<(u32, f32)>,
}

/// Reusable A* scratch; one per querying thread.
#[derive(Default)]
pub struct RouteScratch {
    g: Vec<f32>,
    parent: Vec<u32>,
    stamp: Vec<u32>,
    generation: u32,
    heap: BinaryHeap<Reverse<(u32, u32)>>,
}

/// Work counts of one route query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteStats {
    /// Nodes popped from the open set.
    pub expanded: usize,
}

impl ClusterGraph {
    /// Joins summaries: intra-cell portal costs plus crossings between portals on
    /// shared borders whose intervals overlap at a climbable height.
    pub fn new(grid: CellGrid, summaries: Vec<Arc<ClusterSummary>>) -> Result<Self, NavError> {
        let mut cells = BTreeMap::new();
        for s in summaries {
            let count = s.portals.len();
            if s.costs.len() != count * count
                || s.sample_costs.len() != s.samples.len() * count
                || cells.contains_key(&s.cell)
            {
                return Err(NavError::new(
                    "invalidCluster",
                    "cluster summary sizes disagree or a cell is duplicated",
                ));
            }
            cells.insert(s.cell, (0u32, s));
        }
        let mut positions = Vec::new();
        for (first, summary) in cells.values_mut() {
            *first = positions.len() as u32;
            positions.extend(summary.portals.iter().map(|p| p.position));
        }
        let mut list: Vec<(u32, u32, f32)> = Vec::new();
        let climb = grid.settings.agent.climb.max(0.25) + 0.5;
        for (cell, (first, s)) in &cells {
            let count = s.portals.len();
            for i in 0..count {
                for j in 0..count {
                    let c = s.costs[i * count + j];
                    if i != j && c >= 0.0 {
                        list.push((first + i as u32, first + j as u32, c));
                    }
                }
            }
            for (side, other_side, neighbor) in [
                (
                    Side::PosX,
                    Side::NegX,
                    CellCoord {
                        x: cell.x + 1,
                        z: cell.z,
                    },
                ),
                (
                    Side::PosZ,
                    Side::NegZ,
                    CellCoord {
                        x: cell.x,
                        z: cell.z + 1,
                    },
                ),
            ] {
                let Some((other_first, other)) = cells.get(&neighbor) else {
                    continue;
                };
                for (i, p) in s.portals.iter().enumerate().filter(|(_, p)| p.side == side) {
                    for (j, q) in other
                        .portals
                        .iter()
                        .enumerate()
                        .filter(|(_, q)| q.side == other_side)
                    {
                        let overlap = p.from < q.to - 0.01 && q.from < p.to - 0.01;
                        if overlap && (p.position[1] - q.position[1]).abs() <= climb {
                            let c = distance(p.position, q.position);
                            let (a, b) = (first + i as u32, other_first + j as u32);
                            list.push((a, b, c));
                            list.push((b, a, c));
                        }
                    }
                }
            }
        }
        list.sort_by_key(|a| (a.0, a.1));
        let mut offsets = vec![0u32; positions.len() + 1];
        for &(from, _, _) in &list {
            offsets[from as usize + 1] += 1;
        }
        for i in 0..positions.len() {
            offsets[i + 1] += offsets[i];
        }
        Ok(Self {
            grid,
            cells,
            positions,
            offsets,
            edges: list.into_iter().map(|(_, to, c)| (to, c)).collect(),
        })
    }

    /// Cell grid the graph was built on.
    pub fn grid(&self) -> &CellGrid {
        &self.grid
    }
    /// Number of portals (graph nodes).
    pub fn portal_count(&self) -> usize {
        self.positions.len()
    }
    /// Number of directed edges.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
    /// Summarised cells.
    pub fn cells(&self) -> impl Iterator<Item = &CellCoord> {
        self.cells.keys()
    }

    /// A* from `start` to `goal` through portals. `Ok(None)` when no route exists;
    /// errors when either point lies in an unsummarised cell or off its samples.
    pub fn route(
        &self,
        start: [f32; 3],
        goal: [f32; 3],
        scratch: &mut RouteScratch,
    ) -> Result<(Option<Route>, RouteStats), NavError> {
        let cell = |p: [f32; 3], what: &str| {
            self.cells.get(&self.grid.cell_of(p)).ok_or_else(|| {
                NavError::new(
                    "unknownCell",
                    format!("{what} lies in a cell without a summary"),
                )
            })
        };
        let (start_first, start_summary) = cell(start, "start")?;
        let (goal_first, goal_summary) = cell(goal, "goal")?;
        let (Some((ss, sd)), Some((gs, gd))) =
            (start_summary.attach(start), goal_summary.attach(goal))
        else {
            return Err(NavError::new(
                "offMesh",
                "start or goal has no walkable sample in its cell",
            ));
        };
        let n = self.positions.len();
        let (start_node, goal_node) = (n as u32, n as u32 + 1);
        scratch.generation = scratch.generation.wrapping_add(1);
        if scratch.generation == 0 || scratch.stamp.len() < n + 2 {
            scratch.stamp.clear();
            scratch.stamp.resize(n + 2, 0);
            scratch.g.resize(n + 2, 0.0);
            scratch.parent.resize(n + 2, 0);
            scratch.generation = 1;
        }
        scratch.heap.clear();
        let generation = scratch.generation;
        let goal_count = goal_summary.portals.len();
        let start_count = start_summary.portals.len();
        let goal_cost = |node: u32| -> Option<f32> {
            let k = node.checked_sub(*goal_first)? as usize;
            (k < goal_count)
                .then(|| goal_summary.sample_costs[gs * goal_count + k])
                .filter(|c| *c >= 0.0)
                .map(|c| c + gd)
        };
        let position = |node: u32| -> [f32; 3] {
            if node == start_node {
                start
            } else if node == goal_node {
                goal
            } else {
                self.positions[node as usize]
            }
        };
        let mut stats = RouteStats::default();
        let relax = |scratch: &mut RouteScratch, from: u32, to: u32, cost: f32| {
            let g = scratch.g[from as usize] + cost;
            let slot = to as usize;
            if scratch.stamp[slot] != generation || g < scratch.g[slot] {
                scratch.stamp[slot] = generation;
                scratch.g[slot] = g;
                scratch.parent[slot] = from;
                let f = g + distance(position(to), goal);
                scratch.heap.push(Reverse((f.to_bits(), to)));
            }
        };
        scratch.stamp[start_node as usize] = generation;
        scratch.g[start_node as usize] = 0.0;
        scratch
            .heap
            .push(Reverse((distance(start, goal).to_bits(), start_node)));
        let same_cell = std::ptr::eq(start_summary.as_ref(), goal_summary.as_ref());
        while let Some(Reverse((f_bits, node))) = scratch.heap.pop() {
            let slot = node as usize;
            let g = scratch.g[slot];
            if f32::from_bits(f_bits) > g + distance(position(node), goal) + 1e-3 {
                continue; // Stale heap entry.
            }
            stats.expanded += 1;
            if node == goal_node {
                let mut waypoints = vec![goal];
                let mut at = scratch.parent[slot];
                while at != start_node {
                    waypoints.push(self.positions[at as usize]);
                    at = scratch.parent[at as usize];
                }
                waypoints.reverse();
                return Ok((
                    Some(Route {
                        waypoints,
                        next: 0,
                        cost: g,
                    }),
                    stats,
                ));
            }
            if node == start_node {
                for k in 0..start_count {
                    let c = start_summary.sample_costs[ss * start_count + k];
                    if c >= 0.0 {
                        relax(scratch, node, start_first + k as u32, c + sd);
                    }
                }
                if same_cell {
                    relax(scratch, node, goal_node, distance(start, goal));
                }
                continue;
            }
            for &(to, cost) in
                &self.edges[self.offsets[slot] as usize..self.offsets[slot + 1] as usize]
            {
                relax(scratch, node, to, cost);
            }
            if let Some(c) = goal_cost(node) {
                relax(scratch, node, goal_node, c);
            }
        }
        Ok((None, stats))
    }
}
