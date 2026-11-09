//! Side-cars in the existing explicit worker memo service. Charts use decoded
//! final LOD0 geometry, exactly the native trace input after index optimization.
use super::{
    Vertex,
    patches::{self, PatchCache},
};
use ara_asset::{Artifact, ArtifactKind, CacheBackend, Digest, ImportOutput, Result};
use ara_patch_charts::{Charts, Settings, cache::Key};

struct Memo<'a>(&'a dyn CacheBackend);
impl PatchCache for Memo<'_> {
    fn read(&self, key: Key) -> Result<Option<Vec<u8>>> {
        Ok(self
            .0
            .load(Digest::of(key.relative_path().as_bytes()))?
            .and_then(|hit| {
                hit.output
                    .artifacts
                    .into_iter()
                    .find(|a| a.name == "charts.bin")
                    .map(|a| a.bytes)
            }))
    }
    fn write(&self, key: Key, bytes: &[u8]) -> Result<()> {
        self.0.store(
            Digest::of(key.relative_path().as_bytes()),
            &ImportOutput {
                artifacts: vec![Artifact {
                    name: "charts.bin".into(),
                    kind: ArtifactKind::Custom,
                    bytes: bytes.to_vec(),
                }],
            },
        )
    }
}

fn decoded(bytes: &[u8]) -> Result<(Vec<Vertex>, Vec<u32>)> {
    let mesh = ara_asset_format::Mesh::read(bytes).map_err(crate::invalid)?;
    let bounds: Vec<_> = mesh
        .bounds
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect();
    let vertices = mesh
        .vertices
        .as_chunks::<52>()
        .0
        .iter()
        .map(|v| {
            let position = std::array::from_fn(|axis| {
                let q = u16::from_le_bytes([v[axis * 2], v[axis * 2 + 1]]);
                bounds[axis] + f32::from(q) / 65535. * (bounds[axis + 3] - bounds[axis])
            });
            Vertex {
                position,
                ..Vertex::default()
            }
        })
        .collect();
    let words: Vec<_> = mesh.lods[..16]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_le_bytes(*v))
        .collect();
    let first = words[0] as usize;
    let count = words[1] as usize;
    let indices = mesh.indices[first * 4..(first + count) * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_le_bytes(*v))
        .collect();
    Ok((vertices, indices))
}

/// Old-project load path: missing or corrupt charts rebuild from final native
/// LOD0 order at the actual signed instance scale. Cache I/O is caller-owned.
pub fn load_patch_charts(
    bytes: &[u8],
    material: u32,
    albedo: [f32; 3],
    scale: [f32; 3],
    cache: &impl PatchCache,
) -> Result<Charts> {
    let (vertices, indices) = decoded(bytes)?;
    patches::load_or_build(
        &vertices,
        &indices,
        material,
        albedo,
        Settings {
            scale,
            ..Settings::default()
        },
        cache,
    )
}

/// Both geometry memo hits and new imports populate unit-scale geometry side-cars.
/// Neutral albedo/material key the import variant; material/scale-specific loads
/// rekey explicitly. Side-cars never alter final artifact bytes or importer keys.
pub(crate) fn import_artifacts(
    memo: Option<&dyn CacheBackend>,
    artifacts: &[Artifact],
) -> Result<()> {
    let Some(memo) = memo else {
        return Ok(());
    };
    let cache = Memo(memo);
    for artifact in artifacts.iter().filter(|a| a.kind == ArtifactKind::Mesh) {
        let (vertices, indices) = decoded(&artifact.bytes)?;
        patches::import_charts(&vertices, &indices, 0, [1.; 3], &cache)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct Cache(RefCell<std::collections::BTreeMap<Key, Vec<u8>>>);
    impl PatchCache for Cache {
        fn read(&self, key: Key) -> Result<Option<Vec<u8>>> {
            Ok(self.0.borrow().get(&key).cloned())
        }
        fn write(&self, key: Key, bytes: &[u8]) -> Result<()> {
            self.0.borrow_mut().insert(key, bytes.to_vec());
            Ok(())
        }
    }
    #[test]
    fn patch_service_load_uses_final_lod0_and_reuses_cache() -> Result<()> {
        let vertices = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]].map(|position| Vertex {
            position,
            normal: [0., 0., 1.],
            ..Vertex::default()
        });
        let bytes =
            super::super::cook_mesh(&vertices, &[0, 1, 2], super::super::MeshSettings::default())?;
        let cache = Cache::default();
        let a = load_patch_charts(&bytes, 3, [0.5; 3], [1.; 3], &cache)?;
        let b = load_patch_charts(&bytes, 3, [0.5; 3], [1.; 3], &cache)?;
        assert_eq!(a, b);
        assert_eq!(cache.0.borrow().len(), 1);
        assert!(a.fragments.iter().all(|f| f.triangle == 0));
        Ok(())
    }
}
