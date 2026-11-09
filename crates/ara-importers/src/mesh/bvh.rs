//! Import-time CWBVH build for one mesh asset (A1 track; consumed by `look::trace`).
//!
//! The renderer keys BLASes by the content hash of the geometry it draws
//! (`ara_bvh::Mesh::hash` over positions, indices and submesh starts), so an import
//! build pays off when the import worker hands it those exact bytes: the finest
//! LOD's final positions and indices, in draw order, after every remap. Meshes whose
//! runtime positions differ (node transforms baked at load, skinning) miss this
//! side-car and build once on load through `look::trace::BlasCache` instead; old
//! projects take that path for every mesh. No shared asset-format change.

// Wave 1 provides the hook; the import worker's call site arrives with wave 2.
#![allow(dead_code)]

use super::Vertex;
use ara_asset::{Error, Result};
use ara_bvh::{Blas, Mesh, MeshHash, sidecar};

/// Explicit worker-owned side-car service (the project's `.ara/gpu/bvh/` cache).
/// Implementations publish writes atomically and stay under the project root.
pub(crate) trait BvhCache {
    /// The bytes stored under `name` (`ara_bvh::sidecar::file_name`), if any.
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>>;
    /// Stores `bytes` under `name`.
    fn write(&self, name: &str, bytes: &[u8]) -> Result<()>;
}

/// The BLAS of the mesh with `vertices` and `indices` (submesh starts as the
/// renderer splits it; empty for one), read from `cache` when a valid side-car for
/// these exact bytes exists, else built and written. Pure apart from the cache.
pub(crate) fn import_blas(
    vertices: &[Vertex],
    indices: &[u32],
    submeshes: &[u32],
    cache: &impl BvhCache,
) -> Result<Blas> {
    let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
    let mesh = Mesh {
        positions: &positions,
        indices,
        submeshes,
    };
    let hash: MeshHash = mesh.hash();
    let name = sidecar::file_name(hash);
    if let Some(bytes) = cache.read(&name)?
        && let Ok(blas) = sidecar::decode(&bytes, hash)
    {
        return Ok(blas);
    }
    let blas = Blas::build(&mesh).map_err(|e| Error::Invalid(format!("trace BLAS: {e}")))?;
    cache.write(&name, &sidecar::encode(&blas))?;
    Ok(blas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, Vec<u8>>>);

    impl BvhCache for Memory {
        fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.borrow().get(name).cloned())
        }
        fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
            self.0.borrow_mut().insert(name.into(), bytes.to_vec());
            Ok(())
        }
    }

    #[test]
    fn import_builds_once_and_matches_the_renderer_key() -> Result<()> {
        let vertex = |p: [f32; 3]| Vertex {
            position: p,
            ..Vertex::default()
        };
        let vertices = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]].map(vertex);
        let indices = [0, 1, 2, 2, 1, 3];
        let cache = Memory::default();
        let built = import_blas(&vertices, &indices, &[], &cache)?;
        let positions = vertices.map(|v| v.position);
        let mesh = Mesh {
            positions: &positions,
            indices: &indices,
            submeshes: &[],
        };
        assert_eq!(built.hash, mesh.hash(), "the renderer's on-load key");
        assert_eq!(cache.0.borrow().len(), 1);
        // A corrupt side-car is rebuilt, not trusted.
        for bytes in cache.0.borrow_mut().values_mut() {
            let n = bytes.len();
            bytes[n - 1] ^= 1;
        }
        assert_eq!(import_blas(&vertices, &indices, &[], &cache)?, built);
        assert_eq!(import_blas(&vertices, &indices, &[], &cache)?, built);
        assert!(import_blas(&vertices, &[0, 1, 9], &[], &cache).is_err());
        Ok(())
    }
}
