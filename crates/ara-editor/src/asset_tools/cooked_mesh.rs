//! FBX thumbnail geometry comes from the isolated importer's checked ARAF output.
use ara_asset::{CacheBackend, FileCache, SourceAsset};
use ara_frame::scene::{Mesh, Vertex};
use std::path::Path;

/// Resolve storage input against the same authored boundary used by snapshots.
/// Thumbnail/cache workers supply physical keys from validated current projects.
fn source_path(root: &Path, storage: &str) -> anyhow::Result<String> {
    let authored = ara_host::project::authored_root(root);
    let prefix = authored
        .strip_prefix(root)?
        .to_string_lossy()
        .replace('\\', "/");
    let mut workspace = ara_doc::Workspace::default();
    workspace.set_reference_root(&prefix);
    super::paths::source(&workspace, storage)
        .map(str::to_owned)
        .map_err(anyhow::Error::msg)
}

fn decode(bytes: &[u8], entity: u64, name: String) -> anyhow::Result<Mesh> {
    let mesh = ara_asset_format::Mesh::read(bytes)?;
    let bounds = mesh
        .bounds
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect::<Vec<_>>();
    let vertices = mesh
        .vertices
        .as_chunks::<52>()
        .0
        .iter()
        .map(|bytes| {
            let v = bytemuck::pod_read_unaligned::<ara_asset_format::QuantizedVertex>(bytes);
            let normal: [f32; 3] =
                std::array::from_fn(|axis| (f32::from(v.normal[axis]) / 32767.).max(-1.));
            Vertex {
                position: std::array::from_fn(|axis| {
                    bounds[axis]
                        + f32::from(v.position[axis]) / 65535. * (bounds[axis + 3] - bounds[axis])
                }),
                normal,
                uv: [0.; 2],
                tangent: crate::world::gltf::tangent(glam::Vec3::from(normal).normalize_or_zero()),
            }
        })
        .collect();
    let lod = bytemuck::pod_read_unaligned::<ara_asset_format::Lod>(&mesh.lods[..16]);
    let start = lod.first_index as usize * 4;
    let end = start + lod.index_count as usize * 4;
    let indices = mesh.indices[start..end]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect();
    Ok(Mesh {
        entity,
        name,
        vertices,
        indices,
        material: 0,
    })
}

fn mapping(
    root: &Path,
    logical: &str,
    settings: &ara_asset::ImportSettings,
) -> anyhow::Result<std::path::PathBuf> {
    let value = serde_json::json!({"path":logical,"importer":settings.importer,"version":settings.version,"preset":settings.preset,"settings":settings.settings});
    let key = ara_asset::Digest::of(
        ara_json::to_canonical_string(&ara_json::sort_keys(&value)).as_bytes(),
    );
    Ok(root
        .join(".ara/cache/editor-meshes")
        .join(format!("{key}.json")))
}
/// Publish the derived cache mapping for SourceAsset's authored-relative path.
pub(super) fn publish(
    root: &Path,
    source: &SourceAsset,
    key: ara_asset::Digest,
) -> anyhow::Result<()> {
    let settings = ara_asset::ImportSettings {
        importer: source.importer.clone(),
        version: source.version.clone(),
        preset: source.preset,
        settings: source.settings.clone(),
        dependencies: Vec::new(),
    };
    let path = mapping(root, &source.path, &settings)?;
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("mesh cache parent"))?,
    )?;
    std::fs::write(
        path,
        ara_json::to_canonical_string(&ara_json::to_value(&key)?),
    )?;
    Ok(())
}
/// Read the derived mesh cache for a project-relative storage media path, off-frame.
pub(crate) fn cached(
    root: &Path,
    path: &str,
    settings: &ara_asset::ImportSettings,
) -> anyhow::Result<Option<Vec<Mesh>>> {
    let logical = source_path(root, path)?;
    let path = mapping(root, &logical, settings)?;
    let Some(key) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| ara_json::parse(&text).ok())
        .and_then(|v| serde_json::from_value::<ara_asset::Digest>(v).ok())
    else {
        return Ok(None);
    };
    let Some(cached) = FileCache::new(root)?.load(key)? else {
        return Ok(None);
    };
    let mut meshes = Vec::new();
    for artifact in cached
        .output
        .artifacts
        .iter()
        .filter(|a| a.kind == ara_asset::ArtifactKind::Mesh)
    {
        meshes.push(decode(&artifact.bytes, 1, artifact.name.clone())?);
    }
    Ok(Some(meshes))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thumbnail_storage_paths_convert_once_and_preserve_real_game_folders() -> anyhow::Result<()> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = crate::test_target()
            .join("editor-mesh-source-paths")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        for (name, storage, logical) in [
            (
                "modern",
                format!("{}/assets/model.fbx", ara_doc::AUTHORING_ROOT),
                "assets/model.fbx",
            ),
            (
                "modern-nested",
                format!("{}/game/assets/model.fbx", ara_doc::AUTHORING_ROOT),
                "game/assets/model.fbx",
            ),
        ] {
            let root = base.join(name);
            std::fs::create_dir_all(&root)?;
            let marker = serde_json::json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION});
            std::fs::write(
                root.join("ara.project.json"),
                ara_json::to_canonical_string(&marker),
            )?;
            assert_eq!(source_path(&root, &storage)?, logical);
            assert_eq!(
                ara_host::project::authored_root(&root).join(logical),
                root.join(&storage)
            );
            assert!(source_path(&root, "settings/model.fbx").is_err());
            assert!(source_path(&root, "game/assets/model.fbx").is_err());
            assert!(source_path(&root, "../model.fbx").is_err());
        }
        Ok(())
    }
    #[test]
    fn decodes_the_importers_finest_lod() {
        let input = vec![
            ara_importers::Vertex {
                position: [0., 0., 0.],
                normal: [0., 0., 1.],
                ..Default::default()
            },
            ara_importers::Vertex {
                position: [2., 0., 0.],
                normal: [0., 0., 1.],
                ..Default::default()
            },
            ara_importers::Vertex {
                position: [0., 2., 0.],
                normal: [0., 0., 1.],
                ..Default::default()
            },
        ];
        let bytes = ara_importers::cook_mesh(
            &input,
            &[0, 1, 2],
            ara_importers::MeshSettings {
                lod_count: 1,
                ..Default::default()
            },
        )
        .expect("triangle cook");
        let mesh = decode(&bytes, 1, "Triangle".into()).expect("valid cooked geometry");
        assert_eq!(mesh.indices.len(), 3);
        assert_eq!(mesh.vertices.len(), 3);
        assert!(mesh.vertices.iter().any(|v| v.position[0] > 1.99));
    }
}
