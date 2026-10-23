//! Independent query scratch over an immutable tiled navmesh.
use crate::{BakeArtifact, CacheKey, NavError, OffMeshLink, finite, native};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Area masks and traversal costs; ID 0 is the default walkable area.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QueryFilter {
    /// Per-area costs, each finite and at least 1 (keeps the A* heuristic admissible).
    pub costs: [f32; 16],
    /// Allowed area bits.
    pub include: u16,
    /// Forbidden area bits; takes precedence over include.
    pub exclude: u16,
}
impl Default for QueryFilter {
    fn default() -> Self {
        Self {
            costs: [1.0; 16],
            include: u16::MAX,
            exclude: 0,
        }
    }
}
impl QueryFilter {
    /// Checks costs are finite and in 1..=1000000 (admissible A*).
    pub fn validate(&self) -> Result<(), NavError> {
        self.native(&[]).map(|_| ())
    }
    /// Native filter borrowing `blocked` (ascending link IDs) for one call.
    pub(crate) fn native(&self, blocked: &[u32]) -> Result<ara_nav_sys::Filter, NavError> {
        if self
            .costs
            .iter()
            .any(|c| !c.is_finite() || *c < 1.0 || *c > 1_000_000.0)
        {
            return Err(NavError::new(
                "invalidFilter",
                "area costs must be finite and in 1..=1000000",
            ));
        }
        Ok(ara_nav_sys::Filter {
            costs: self.costs,
            include: self.include,
            exclude: self.exclude,
            blocked: if blocked.is_empty() {
                std::ptr::null()
            } else {
                blocked.as_ptr()
            },
            blocked_count: blocked.len().min(i32::MAX as usize) as i32,
        })
    }
    /// Whether a particular area can be traversed.
    pub fn allows(&self, area: u8) -> bool {
        area < 16 && self.include & (1 << area) != 0 && self.exclude & (1 << area) == 0
    }
}

struct MeshInner {
    native: native::Mesh,
    profile: crate::AgentProfile,
    hash: CacheKey,
    links: Vec<OffMeshLink>,
    walls: ara_crowd::Walls,
}
/// Wall index cell size: covers the crowd wall look-ahead at up to 9 m/s.
const WALL_CELL: f32 = 4.0;

/// Cheap shared handle to a fully assembled immutable tiled mesh.
#[derive(Clone)]
pub struct NavMesh {
    inner: Arc<MeshInner>,
}
impl std::fmt::Debug for NavMesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavMesh")
            .field("hash", &self.inner.hash)
            .finish()
    }
}
impl NavMesh {
    /// Assembles an artifact OFFLINE/ON A WORKER before a tick-boundary publication.
    pub fn from_artifact(artifact: &BakeArtifact) -> Result<Self, NavError> {
        let native = native::Mesh::new(artifact)?;
        // Boundary edges for crowd avoidance, in deterministic tile/polygon order.
        let walls: Vec<_> = native
            .walls()
            .into_iter()
            .map(|w| ara_crowd::Wall { a: w.a, b: w.b })
            .collect();
        let walls = ara_crowd::Walls::new(&walls, WALL_CELL)
            .map_err(|e| NavError::new("invalidTile", e.0))?;
        Ok(Self {
            inner: Arc::new(MeshInner {
                native,
                profile: artifact.settings.agent,
                hash: artifact.content_hash(),
                links: artifact.links.clone(),
                walls,
            }),
        })
    }
    /// Walkable-surface boundary for crowd avoidance (built with the mesh).
    pub fn walls(&self) -> &ara_crowd::Walls {
        &self.inner.walls
    }
    /// Authored links baked into this revision, sorted by ID.
    pub fn links(&self) -> &[OffMeshLink] {
        &self.inner.links
    }
    /// Content identity suitable for replay/version checks.
    pub fn content_hash(&self) -> CacheKey {
        self.inner.hash
    }
    /// Agent dimensions used to bake this mesh.
    pub fn profile(&self) -> crate::AgentProfile {
        self.inner.profile
    }
    /// Allocates isolated query scratch (maximum 65535 search nodes).
    pub fn query(&self) -> Result<NavQuery<'_>, NavError> {
        Ok(NavQuery {
            native: self.inner.native.query(16384)?,
            extent: [2.0, 4.0, 2.0],
            capacity: 4096,
            blocked: Vec::new(),
        })
    }
    /// Batches paths on the caller's Rayon pool, retaining input/result order.
    /// Each worker owns its query scratch; the shared mesh is never locked.
    pub fn paths(&self, requests: &[PathRequest]) -> Vec<Result<NavPath, NavError>> {
        self.paths_blocked(requests, &[])
    }
    /// Like [`NavMesh::paths`], never traversing the `blocked` off-mesh link IDs.
    pub fn paths_blocked(
        &self,
        requests: &[PathRequest],
        blocked: &[u32],
    ) -> Vec<Result<NavPath, NavError>> {
        // Each chunk allocates one query; keep chunks large but spread small batches.
        let chunk = (requests.len() / (rayon::current_num_threads() * 2)).clamp(16, 256);
        let chunks: Vec<_> = requests
            .par_chunks(chunk)
            .map(|chunk| {
                let mut query = self.query();
                if let Ok(q) = &mut query {
                    q.set_blocked_links(blocked);
                }
                chunk
                    .iter()
                    .map(|r| match &mut query {
                        Ok(q) => q.path(r.start, r.end, &r.filter),
                        Err(e) => Err(e.clone()),
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        chunks.into_iter().flatten().collect()
    }
    pub(crate) fn native(&self) -> &native::Mesh {
        &self.inner.native
    }
}

/// Thread-local mutable scratch borrowing an immutable mesh.
pub struct NavQuery<'a> {
    native: native::Query<'a>,
    extent: [f32; 3],
    capacity: usize,
    blocked: Vec<u32>,
}
impl NavQuery<'_> {
    /// Off-mesh link IDs that paths and visibility must not traverse (closed doors,
    /// absent elevators). Any order; stored ascending without duplicates.
    pub fn set_blocked_links(&mut self, links: &[u32]) {
        self.blocked.clear();
        self.blocked.extend_from_slice(links);
        self.blocked.sort_unstable();
        self.blocked.dedup();
    }
    /// Sets nearest-point search half extents; Y may differ from the horizontal radius.
    pub fn set_extent(&mut self, extent: [f32; 3]) -> Result<(), NavError> {
        if !finite(extent) || extent.iter().any(|v| *v <= 0.0 || *v > 10000.0) {
            return Err(NavError::new(
                "invalidExtent",
                "search extent must be finite and positive",
            ));
        }
        self.extent = extent;
        Ok(())
    }
    /// Sets corridor/corner capacity (16..=32768); truncation is reported in paths.
    pub fn set_capacity(&mut self, capacity: usize) -> Result<(), NavError> {
        if !(16..=32768).contains(&capacity) {
            return Err(NavError::new(
                "capacity",
                "query capacity must be in 16..=32768",
            ));
        }
        self.capacity = capacity;
        Ok(())
    }
    /// Finds the closest allowed mesh point within the configured search extents.
    pub fn nearest(
        &mut self,
        point: [f32; 3],
        filter: &QueryFilter,
    ) -> Result<Option<NearestPoint>, NavError> {
        check_points(point, point)?;
        let f = filter.native(&self.blocked)?;
        Ok(self
            .native
            .nearest(point, self.extent, &f)
            .map(|(polygon, position)| NearestPoint { position, polygon }))
    }
    /// A* corridor followed by funnel straight path. Partial paths retain the
    /// nearest reachable endpoint and never silently count as a successful goto.
    pub fn path(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        filter: &QueryFilter,
    ) -> Result<NavPath, NavError> {
        check_points(start, end)?;
        let result = self.native.path(
            start,
            end,
            self.extent,
            &filter.native(&self.blocked)?,
            self.capacity,
        )?;
        let corners = result
            .corners
            .iter()
            .map(|c| PathCorner {
                position: c.position,
                off_mesh: c.flags & 4 != 0,
                polygon: c.polygon,
            })
            .collect();
        Ok(NavPath {
            corners,
            complete: result.complete,
            reachable: result.reachable,
            truncated: result.status & ((1 << 4) | (1 << 5)) != 0,
        })
    }
    /// Casts across walkable polygons; does not traverse off-mesh connections.
    pub fn raycast(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        filter: &QueryFilter,
    ) -> Result<RaycastHit, NavError> {
        check_points(start, end)?;
        let (fraction, normal) =
            self.native
                .raycast(start, end, self.extent, &filter.native(&self.blocked)?)?;
        Ok(RaycastHit {
            fraction,
            normal,
            position: std::array::from_fn(|i| start[i] + (end[i] - start[i]) * fraction),
        })
    }
    /// Looks up a polygon's area; stale or zero references return None.
    pub fn polygon_area(&mut self, polygon: u32) -> Option<u8> {
        self.native.area(polygon)
    }
    /// Tests direct walkable visibility, including destination floor identity.
    /// Unlike an XZ raycast, this cannot shortcut between stacked surfaces.
    pub fn visible(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        filter: &QueryFilter,
    ) -> Result<bool, NavError> {
        check_points(start, end)?;
        Ok(self
            .native
            .visible(start, end, self.extent, &filter.native(&self.blocked)?))
    }
}
fn check_points(a: [f32; 3], b: [f32; 3]) -> Result<(), NavError> {
    if !finite(a) || !finite(b) || a.into_iter().chain(b).any(|v| v.abs() > 1_000_000.0) {
        Err(NavError::new(
            "invalidPoint",
            "positions must be finite and within one million metres",
        ))
    } else {
        Ok(())
    }
}

/// Input to an ordered batch path query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathRequest {
    /// Requested starting point.
    pub start: [f32; 3],
    /// Requested goal.
    pub end: [f32; 3],
    /// Agent's allowed areas and costs.
    pub filter: QueryFilter,
}
/// Closest point on one allowed polygon. References belong to the queried mesh revision.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NearestPoint {
    /// World-space projected position.
    pub position: [f32; 3],
    /// Opaque native polygon reference, valid only for this mesh revision.
    pub polygon: u32,
}
/// Funnel waypoint, optionally the start of an off-mesh traversal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PathCorner {
    /// World-space waypoint.
    pub position: [f32; 3],
    /// This point starts an off-mesh connection; the next point is its endpoint.
    pub off_mesh: bool,
    /// Polygon entered here (zero on the terminal corner).
    pub polygon: u32,
}
/// Corridor/funnel result, including explicit partial/truncated state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavPath {
    /// Ordered straight-path corners, including area crossings.
    pub corners: Vec<PathCorner>,
    /// True only when the destination polygon is reached without capacity exhaustion.
    pub complete: bool,
    /// Closest reachable point to the projected goal.
    pub reachable: [f32; 3],
    /// Search nodes or corridor/corner capacity were exhausted.
    pub truncated: bool,
}
/// Surface raycast result; fraction 1 means the horizontal segment is unobstructed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RaycastHit {
    /// Fraction along the requested segment, clamped to 0..1.
    pub fraction: f32,
    /// Wall normal, or zero when unobstructed.
    pub normal: [f32; 3],
    /// Interpolated hit position (Y is interpolated, not sampled from the surface).
    pub position: [f32; 3],
}
