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
    let prepared = cache::prepare(mesh, settings, bytes.as_deref()).map_err(|e| {
        Error::Invalid(match e {
            ara_patch_charts::Error::FragmentLimit => too_large(mesh, settings),
            e => e.to_string(),
        })
    })?;
    if let Some(bytes) = prepared.write_back {
        cache.write(prepared.key, &bytes)?;
    }
    Ok(prepared.charts)
}

/// What to tell the author when a mesh's surface is more than the chart budget covers: how
/// much surface it has, how much fits, and what to do. The limit itself says none of that.
fn too_large(mesh: Mesh<'_>, settings: Settings) -> String {
    let scale = settings.scale.map(f64::from);
    let at = |index: u32| {
        let p = mesh.positions[index as usize];
        [0, 1, 2].map(|axis| f64::from(p[axis]) * scale[axis])
    };
    let area: f64 = mesh
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.vertices.map(at);
            let (u, v) = (
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            let cross = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            0.5 * (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt()
        })
        .sum();
    // A flat square metre is cut into two fragments per patch cell.
    let edge = f64::from(settings.edge);
    let fits = f64::from(settings.max_fragments) * edge * edge / 2.0;
    format!(
        "patch charts: the mesh has about {area:.0} m2 of surface, more than {} fragments of {edge} m patches can cover (about {fits:.0} m2 when flat). Split it into several meshes, or author it smaller and scale the entity.",
        settings.max_fragments
    )
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

    #[test]
    fn a_mesh_too_large_for_its_charts_says_how_large_and_what_to_do() {
        // A 10 m square (two triangles) against a budget of 1,000 fragments of 0.25 m patches.
        let vertices =
            [[0., 0., 0.], [10., 0., 0.], [10., 0., 10.], [0., 0., 10.]].map(|position| Vertex {
                position,
                ..Vertex::default()
            });
        let error = load_or_build(
            &vertices,
            &[0, 2, 1, 0, 3, 2],
            0,
            [0.5; 3],
            Settings {
                max_fragments: 1000,
                ..Settings::default()
            },
            &MemoryCache::default(),
        )
        .unwrap_err()
        .to_string();
        for part in [
            "about 100 m2 of surface",
            "1000 fragments of 0.25 m",
            "about 31 m2",
            "scale the entity",
        ] {
            assert!(error.contains(part), "{error}");
        }
    }
}
