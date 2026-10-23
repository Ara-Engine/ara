#![warn(missing_docs)]
//! Virtual shadow address spaces and frame-boundary cache publication.

use glam::Mat4;
use serde::Serialize;

/// Resolution of each virtual projection.
pub const VIRTUAL_SIZE: u32 = 16384;
/// Texels along a physical/virtual page edge.
pub const PAGE_SIZE: u32 = 128;
/// Virtual page rows and columns.
pub const PAGE_SIDE: u32 = VIRTUAL_SIZE / PAGE_SIZE;

/// Unique shadow projection domain, independent of physical residency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Projection {
    /// Directional clipmap level.
    Directional(u8),
    /// Spot light view.
    Spot,
    /// Point light cube face (0..6).
    Cube(u8),
}
/// One virtual page; moving clipmap origins use signed world-page coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct PageKey {
    /// Stable light identity.
    pub light: u64,
    /// Projection/face/clipmap identity.
    pub projection: Projection,
    /// World-page coordinates for directional lights, virtual UV pages otherwise.
    pub xy: [i32; 2],
}
/// Immutable projection extraction; matrices use reversed-Z.
#[derive(Clone, Copy)]
pub struct Light {
    /// Stable scene identity.
    pub id: u64,
    /// Projection domain.
    pub projection: Projection,
    /// World-to-clip matrix.
    pub vp: Mat4,
    /// Changes when light orientation/projection changes, not camera panning.
    pub revision: u64,
}
/// A dirty page assignment awaiting completed GPU rendering.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct DirtyPage {
    /// Requested virtual page.
    pub key: PageKey,
    /// Physical page slot.
    pub slot: u32,
    /// Assignment identity; stale completions must not publish replacement pages.
    pub generation: u64,
}
mod cache;
mod projection;
pub use cache::Cache;
pub use projection::{directional, point, spot};
#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    fn key(x: i32) -> PageKey {
        PageKey {
            light: 1,
            projection: Projection::Directional(0),
            xy: [x, 0],
        }
    }
    #[test]
    fn cache_publication_movement_and_stale_completion() -> Result<()> {
        let mut c = Cache::new(2)?;
        c.begin(1, &[])?;
        let p = c.request([key(0), key(1)])?;
        assert_eq!(p.len(), 2);
        assert!(c.resident(key(0)).is_none());
        assert!(c.complete(p[0]));
        assert!(c.complete(p[1]));
        c.begin(2, &[])?;
        assert!(c.request([key(0)])?.is_empty());
        let new = c.request([key(2)])?;
        assert_eq!(new.len(), 1);
        assert!(c.resident(key(1)).is_none());
        assert!(!c.complete(p[1]));
        c.invalidate([key(0)]);
        assert_eq!(c.request([key(0)])?.len(), 1);
        Ok(())
    }
}
