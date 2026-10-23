//! Renderer-independent debug stream; consumers choose display colors and width.
use crate::{NavMesh, NavPath};
use serde::{Deserialize, Serialize};

/// Typed draw command with semantic metadata for renderer and agent captures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DebugPrimitive {
    /// Walkable navmesh polygon triangulation.
    Triangle {
        /// World-space triangle vertices.
        vertices: [[f32; 3]; 3],
        /// Semantic area ID.
        area: u8,
    },
    /// Authored off-mesh connection.
    Link {
        /// Stable authored link ID.
        id: u32,
        /// World-space start.
        start: [f32; 3],
        /// World-space end.
        end: [f32; 3],
        /// Whether the link was enabled for this mesh revision.
        enabled: bool,
    },
    /// Corridor/funnel segment.
    PathSegment {
        /// Segment start.
        start: [f32; 3],
        /// Segment end.
        end: [f32; 3],
        /// Whether this segment traverses an off-mesh connection.
        off_mesh: bool,
    },
    /// Agent collision footprint and current velocity.
    Agent {
        /// Stable simulation ID.
        id: u64,
        /// World-space position.
        position: [f32; 3],
        /// Collision radius.
        radius: f32,
        /// Current world-space velocity.
        velocity: [f32; 3],
    },
}
impl NavMesh {
    /// Extracts optional debug commands without coupling navigation to a renderer.
    pub fn debug_stream(&self, mut emit: impl FnMut(DebugPrimitive)) {
        for t in self.native().debug() {
            emit(DebugPrimitive::Triangle {
                vertices: [t.a, t.b, t.c],
                area: t.area,
            });
        }
        for l in self.links() {
            emit(DebugPrimitive::Link {
                id: l.id,
                start: l.start,
                end: l.end,
                enabled: l.enabled,
            });
        }
    }
}
impl NavPath {
    /// Emits the straight path as semantic debug segments.
    pub fn debug_stream(&self, mut emit: impl FnMut(DebugPrimitive)) {
        for pair in self.corners.windows(2) {
            emit(DebugPrimitive::PathSegment {
                start: pair[0].position,
                end: pair[1].position,
                off_mesh: pair[0].off_mesh,
            });
        }
    }
}

impl NavMesh {
    /// Rasterises this mesh's walkable (radius-eroded) triangles into a grid over
    /// `min`..`max` (XZ) for [`ara_crowd::FlowField::build`]. WORKER/LOAD TIME.
    pub fn walk_grid(
        &self,
        min: [f32; 2],
        max: [f32; 2],
        cell: f32,
    ) -> Result<ara_crowd::WalkGrid, crate::NavError> {
        let mut grid = ara_crowd::WalkGrid::new(min, max, cell)
            .map_err(|e| crate::NavError::new("invalidGrid", e.0))?;
        let triangles: Vec<[[f32; 3]; 3]> = self
            .native()
            .debug()
            .iter()
            .map(|t| [t.a, t.b, t.c])
            .collect();
        grid.rasterize(&triangles, 1);
        Ok(grid)
    }
}
