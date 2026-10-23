//! `RayScene` over `ara-physics` batch ray queries (feature `physics`).
//!
//! Hits arrive sorted by fraction/body/subshape from the backend. `QueryHit::user_data`
//! is the application identity given when the body was created (normally the entity's
//! `PhysicsId`); the runtime maps it to the entity's `AcousticSurface` and label.

use crate::{
    geometry::{RayHit, RayHits, RayScene, Segment},
    material::AcousticMaterial,
    math::sub,
};
use ara_audio::{AudioError, Result};
use ara_physics::{PhysicsBackend, Query, QueryFilter, QueryResults};

/// Maps physics user data to acoustic surfaces and trace labels.
pub trait SurfaceLookup {
    /// Material of the body with this user data (`AcousticSurface::material`, or generic).
    fn material(&self, user_data: u64) -> AcousticMaterial;
    /// Label for traces (`e_wall01`).
    fn label(&self, user_data: u64) -> String {
        format!("#{user_data}")
    }
}

/// Reusable query buffers; keep one per acoustics worker.
#[derive(Default)]
pub struct PhysicsRayBuffers {
    queries: Vec<Query>,
    results: QueryResults,
}

/// A borrowed physics world seen as a `RayScene` for one acoustics update.
pub struct PhysicsScene<'a, B: PhysicsBackend> {
    /// The physics backend (queries are read-only).
    pub backend: &'a B,
    /// Layer mask; usually excludes triggers and characters.
    pub filter: QueryFilter,
    /// Surface materials and labels.
    pub surfaces: &'a dyn SurfaceLookup,
    /// Reused buffers.
    pub buffers: &'a mut PhysicsRayBuffers,
}

impl<B: PhysicsBackend> RayScene for PhysicsScene<'_, B> {
    fn trace(&mut self, segments: &[Segment], out: &mut RayHits) -> Result<()> {
        let buffers = &mut *self.buffers;
        buffers.queries.clear();
        buffers.queries.extend(segments.iter().map(|s| Query::Ray {
            origin: s.from,
            displacement: sub(s.to, s.from),
            filter: self.filter,
        }));
        self.backend
            .queries(&buffers.queries, &mut buffers.results)
            .map_err(|e| AudioError::Backend(format!("physics ray query: {e}")))?;
        out.clear();
        for i in 0..segments.len() {
            out.begin();
            for hit in buffers.results.get(i) {
                out.push(RayHit {
                    fraction: hit.fraction,
                    point: hit.point,
                    normal: hit.normal,
                    surface: hit.user_data,
                    material: self.surfaces.material(hit.user_data),
                });
            }
            out.sort_current();
        }
        Ok(())
    }

    fn label(&self, surface: u64) -> String {
        self.surfaces.label(surface)
    }
}
