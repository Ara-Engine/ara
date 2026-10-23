//! Worker-side physics cooking; the host publishes handles only at allowed ticks.
use crate::{Header, Tile, TileCoord};
use ara_physics::{PhysicsBackend, PhysicsError, Shape, ShapeHandle};

impl Tile {
    /// Cooks native heightfield collision for solid tiles. Cell holes use an exact
    /// triangle mesh because Jolt's vertex-hole sentinel removes neighboring cells.
    /// Completely empty tiles return None so the host can remove their old collider.
    /// Geometry includes the world origin; attach it to a static identity-pose body.
    pub fn cook_collider<B: PhysicsBackend>(
        &self,
        header: &Header,
        coord: TileCoord,
        backend: &mut B,
    ) -> ara_physics::Result<Option<ShapeHandle>> {
        self.validate()
            .and_then(|_| header.validate())
            .map_err(|d| PhysicsError(d.message))?;
        if self.cells != header.cells || !header.contains(coord) {
            return Err(PhysicsError("terrain collider grid mismatch".into()));
        }
        let field = self.heightfield(header, coord);
        if self.holes.contains(&1) {
            let mesh = field.mesh();
            if mesh.triangles.is_empty() {
                return Ok(None);
            }
            let indices: Vec<_> = mesh.triangles.iter().flatten().copied().collect();
            backend
                .create_shape(Shape::Mesh {
                    vertices: &mesh.vertices,
                    indices: &indices,
                })
                .map(Some)
        } else {
            backend
                .create_shape(Shape::HeightField {
                    side: self.cells + 1,
                    heights: &self.heights,
                    scale: [header.spacing, 1., header.spacing],
                    offset: field.origin,
                })
                .map(Some)
        }
    }
}
