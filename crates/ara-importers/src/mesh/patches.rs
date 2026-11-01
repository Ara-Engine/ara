//! Import-time 25 cm surface-patch charts for one mesh asset (A2 track; consumed by `look::transport`).

// The import/load service owns cache I/O. Wave 1 provides the hook without changing
// the shared mesh format or enabling transport in existing rendering defaults.
#![allow(dead_code)]

use super::Vertex;
use ara_asset::{Error, Result};
use ara_patch_charts::{Charts, Mesh, Settings, Triangle, cache};

/// Explicit worker-owned side-car service. Implementations publish writes atomically.
pub trait PatchCache {
    /// Read below a project-owned cache root; never the real user profile.
    fn read(&self, key: cache::Key) -> Result<Option<Vec<u8>>>;
    /// Store the versioned bytes separately from the shared asset format.
    fn write(&self, key: cache::Key, bytes: &[u8]) -> Result<()>;
}

/// Builds the reference-scale chart side-car while importing one material primitive.
/// Use exactly the triangle order supplied to the tracing BLAS. A1's native adapter
/// uses LOD0 indices; if optimization reordered them, chart/rekey that final order
/// rather than interpreting original import triangle ids as native trace ids.
/// This function is pure apart from the explicitly supplied cache service.
pub fn import_charts(
    vertices: &[Vertex],
    indices: &[u32],
    material: u32,
    albedo: [f32; 3],
    cache: &impl PatchCache,
) -> Result<Charts> {
    load_or_build(
        vertices,
        indices,
        material,
        albedo,
        Settings::default(),
        cache,
    )
}

/// Old-project fallback and nonuniform-scale variant. Call on load once scale is
/// known. A unit-scale hit reuses import charts; other scales get their own key.
pub fn load_or_build(
    vertices: &[Vertex],
    indices: &[u32],
    material: u32,
    albedo: [f32; 3],
    settings: Settings,
    cache: &impl PatchCache,
) -> Result<Charts> {
    if !indices.len().is_multiple_of(3) {
        return Err(Error::Invalid(
            "patch charts need complete triangles".into(),
        ));
    }
    let positions: Vec<_> = vertices.iter().map(|v| v.position).collect();
    let triangles: Vec<_> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|v| Triangle {
            vertices: [v[0], v[1], v[2]],
            material,
            albedo,
        })
        .collect();
    let mesh = Mesh {
        positions: &positions,
        triangles: &triangles,
    };
    let key = cache::Key::new(mesh, settings);
    let bytes = cache.read(key)?;
    let prepared = cache::prepare(mesh, settings, bytes.as_deref())
        .map_err(|e| Error::Invalid(e.to_string()))?;
    if let Some(bytes) = prepared.write_back {
        cache.write(prepared.key, &bytes)?;
    }
    Ok(prepared.charts)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::BTreeMap};

    #[derive(Default)]
    struct MemoryCache(RefCell<BTreeMap<cache::Key, Vec<u8>>>);
    impl PatchCache for MemoryCache {
        fn read(&self, key: cache::Key) -> Result<Option<Vec<u8>>> {
            Ok(self.0.borrow().get(&key).cloned())
        }
        fn write(&self, key: cache::Key, bytes: &[u8]) -> Result<()> {
            self.0.borrow_mut().insert(key, bytes.to_vec());
            Ok(())
        }
    }

    #[test]
    fn patches_import_and_legacy_load_share_the_content_cache() {
        let vertices = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]].map(|position| Vertex {
            position,
            ..Vertex::default()
        });
        let cache = MemoryCache::default();
        let imported = import_charts(&vertices, &[0, 1, 2], 2, [0.5; 3], &cache).unwrap();
        let loaded = load_or_build(
            &vertices,
            &[0, 1, 2],
            2,
            [0.5; 3],
            Settings::default(),
            &cache,
        )
        .unwrap();
        assert_eq!(imported, loaded);
        assert_eq!(cache.0.borrow().len(), 1);
        let scaled = load_or_build(
            &vertices,
            &[0, 1, 2],
            2,
            [0.5; 3],
            Settings {
                scale: [2.; 3],
                ..Settings::default()
            },
            &cache,
        )
        .unwrap();
        assert!(scaled.patches.len() > imported.patches.len());
        assert_eq!(cache.0.borrow().len(), 2);
        let legacy = MemoryCache::default();
        assert_eq!(
            load_or_build(
                &vertices,
                &[0, 1, 2],
                2,
                [0.5; 3],
                Settings::default(),
                &legacy
            )
            .unwrap(),
            imported
        );
    }
}
