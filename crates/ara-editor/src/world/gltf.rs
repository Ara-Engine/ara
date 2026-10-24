//! Cooked model access; the viewport never discovers or imports source glTF files.
use anyhow::Context;
use ara_render::scene::{Mesh, Vertex};
use ara_runtime::assets::AssetSource;
use glam::Vec3;
use std::path::Path;

/// Reads the transactional project default, including unsaved typed settings edits.
pub(crate) fn render_quality(
    workspace: &ara_doc::Workspace,
) -> anyhow::Result<ara_runtime_render::QualityChoice> {
    let settings: ara_components::render::RenderSettings = workspace
        .doc("pj_project")
        .and_then(|doc| doc.record("p_settings"))
        .and_then(|record| record.get("settings"))
        .and_then(|settings| settings.get("rendering"))
        .map(serde::Deserialize::deserialize)
        .transpose()?
        .unwrap_or_default();
    Ok(settings.quality.into())
}

pub(crate) fn load(
    path: &Path,
    entity: u64,
    name: String,
    material: usize,
) -> anyhow::Result<Mesh> {
    let root = path
        .ancestors()
        .find(|root| root.join("ara.project.json").is_file())
        .context("model has no project root")?;
    let reader = ara_play_host::PublishedAssets::for_project(root).map_err(anyhow::Error::msg)?;
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(".import.json");
    let value = ara_json::parse(&std::fs::read_to_string(sidecar)?)?;
    let asset = value["id"]
        .as_str()
        .context("model sidecar has no asset ID")?;
    let model = reader
        .snapshot()
        .map(|snapshot| -> anyhow::Result<_> {
            let Some((digest, cached)) = snapshot.load(asset)? else {
                return Ok(None);
            };
            Ok(Some(ara_runtime::model::Model::decode(
                asset, digest, &cached,
            )?))
        })
        .transpose()?
        .flatten();
    let mut mesh = Mesh {
        entity,
        name,
        vertices: Vec::new(),
        indices: Vec::new(),
        material,
    };
    if let Some(model) = model {
        for primitive in &model.primitives {
            let base = u32::try_from(mesh.vertices.len())?;
            mesh.vertices
                .extend(primitive.vertices.iter().map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    uv: v.uv,
                    tangent: v.tangent,
                }));
            mesh.indices
                .extend(primitive.indices.iter().map(|i| base + i));
        }
    } else {
        // The owner publishes asynchronously. Opening a project never waits for it.
        let placeholder =
            ara_runtime_render::geometry::shape(ara_runtime::components::MeshShape::Box, [1.0; 3]);
        mesh.vertices = placeholder
            .vertices
            .iter()
            .map(|v| Vertex {
                position: v.position,
                normal: v.normal,
                uv: v.uv,
                tangent: v.tangent,
            })
            .collect();
        mesh.indices = placeholder.indices;
    }
    anyhow::ensure!(
        !mesh.indices.is_empty(),
        "glTF contains no triangle geometry"
    );
    Ok(mesh)
}

/// Source generations and evaluated terrain retained across editor content revisions.
pub(crate) struct FidelityContent {
    reader: ara_play_host::PublishedAssets,
    snapshot: Option<std::sync::Arc<ara_play_host::PublishedSnapshot>>,
    library: ara_runtime::animation::Library,
    materials: Option<(u64, u64)>,
    terrain: Option<(u64, ara_runtime::terrain::TerrainState)>,
}
impl FidelityContent {
    pub(crate) fn new(root: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            reader: ara_play_host::PublishedAssets::for_project(root)
                .map_err(anyhow::Error::msg)?,
            snapshot: None,
            library: Default::default(),
            materials: None,
            terrain: None,
        })
    }
    pub(crate) fn revision(&self) -> u64 {
        self.reader.revision()
    }
    pub(crate) fn frame(
        &mut self,
        view: &super::DocumentView,
        workspace: &ara_doc::Workspace,
        schema: &ara_schema::SchemaDb,
        preview: Option<&super::gesture::Gesture>,
        hidden: &std::collections::BTreeSet<String>,
    ) -> anyhow::Result<ara_runtime::extract::RenderFrame> {
        use super::EditWorldView;
        use std::hash::{Hash, Hasher};
        if self
            .snapshot
            .as_ref()
            .is_none_or(|old| old.revision() != self.reader.revision())
        {
            self.snapshot = Some(
                self.reader
                    .snapshot()
                    .context("asset publication is not available yet")?,
            );
        }
        let snapshot = self.snapshot.as_ref().context("published asset snapshot")?;
        // Metadata checks avoid FileCache reads/decodes for unchanged models. The
        // library includes foliage/prefab sources, so extraction reuses them too.
        for (asset, generation) in snapshot.generations() {
            if generation.kinds().contains(&ara_asset::ArtifactKind::Mesh)
                && self
                    .library
                    .models
                    .get(asset)
                    .is_none_or(|model| model.digest != generation.digest())
            {
                self.library.models.load(snapshot.as_ref(), asset)?;
            }
        }
        let mut material_hash = std::hash::DefaultHasher::new();
        let mut terrain_hash = std::hash::DefaultHasher::new();
        for (_, document) in workspace.docs() {
            let target = match document.kind.as_str() {
                "graph" | "materialinstance" => &mut material_hash,
                "terrain" | "foliage" => &mut terrain_hash,
                _ => continue,
            };
            document.id.as_str().hash(target);
            ara_doc::write(document, schema).hash(target);
        }
        let materials = (material_hash.finish(), snapshot.revision());
        if self.materials != Some(materials) {
            let requested: Vec<_> = self.library.materials.ids().map(str::to_owned).collect();
            for asset in requested {
                self.library
                    .materials
                    .load(workspace, snapshot.as_ref(), &asset)?;
            }
            self.materials = Some(materials);
        }
        let mut transforms = super::world_transforms(view.entities());
        if let Some(preview) = preview {
            preview.preview_world(view.entities(), &mut transforms);
        }
        for (address, row) in view.entities() {
            if row.record.contains_key("Terrain") {
                address.hash(&mut terrain_hash);
                hidden.contains(address).hash(&mut terrain_hash);
                ara_json::to_canonical_string(&ara_json::sort_keys(&row.record["Terrain"]))
                    .hash(&mut terrain_hash);
                if let Some(matrix) = transforms.get(address) {
                    for value in matrix.to_cols_array() {
                        value.to_bits().hash(&mut terrain_hash);
                    }
                }
            }
        }
        let stamp = terrain_hash.finish();
        let cached = self
            .terrain
            .as_ref()
            .filter(|(old, _)| *old == stamp)
            .map(|(_, data)| data);
        let frame = ara_runtime_render::fidelity::native::document_frame(
            view.entities(),
            &transforms,
            workspace,
            &mut self.library,
            snapshot.as_ref(),
            cached,
            hidden,
        )?;
        self.terrain = frame.terrain.as_ref().map(|terrain| {
            (
                stamp,
                ara_runtime::terrain::TerrainState(std::sync::Arc::clone(&terrain.data)),
            )
        });
        Ok(frame)
    }
    pub(crate) fn content(
        &self,
        frame: &ara_runtime::extract::RenderFrame,
        workspace: &ara_doc::Workspace,
        schema: &ara_schema::SchemaDb,
    ) -> anyhow::Result<ara_runtime_render::fidelity::native::Content> {
        ara_runtime_render::fidelity::native::Content::resolve(
            frame,
            workspace,
            schema,
            &self.library,
            self.snapshot
                .as_deref()
                .context("published asset snapshot")?,
        )
    }
}

/// A unit tangent perpendicular to `normal`, for loaders without UV tangents. A fixed
/// +X tangent is parallel to ±X normals, and the fidelity shading basis
/// (`normalize(t - n·dot(t, n))`) then turns those faces black.
pub(crate) fn tangent(normal: Vec3) -> [f32; 4] {
    normal.any_orthonormal_vector().extend(1.).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tangent_is_perpendicular_to_axis_normals() {
        for normal in [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ] {
            let t = Vec3::from_slice(&tangent(normal)[..3]);
            assert!((t.length() - 1.).abs() < 1e-5, "{normal}");
            assert!(t.dot(normal).abs() < 1e-5, "{normal}");
        }
    }
}
