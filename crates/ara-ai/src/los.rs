//! Navmesh line of sight: a sight line is blocked where it crosses the walkable
//! surface's boundary (walls, carved obstacles, holes) near its height.
use ara_nav::NavMesh;
use ara_perception::LineOfSight;

/// 2.5D line of sight over a navmesh's boundary edges. Allocation-free and
/// thread-safe; use physics rays when low cover should not block sight.
#[derive(Clone)]
pub struct NavLineOfSight {
    /// Mesh whose walls block sight.
    pub mesh: NavMesh,
    /// A wall blocks when the sight line passes within this height of it (m).
    pub vertical: f32,
}
impl NavLineOfSight {
    /// Sight over `mesh` with a 2.5 m wall-height tolerance.
    pub fn new(mesh: NavMesh) -> Self {
        Self {
            mesh,
            vertical: 2.5,
        }
    }
}
impl LineOfSight for NavLineOfSight {
    fn clear(&self, from: [f32; 3], to: [f32; 3], _: u64, _: u64) -> bool {
        self.mesh
            .walls()
            .first_hit(from, to, self.vertical)
            .is_none()
    }
}
