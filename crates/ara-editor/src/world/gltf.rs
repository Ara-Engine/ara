//! Cooked model access; the viewport never discovers or imports source glTF files.
use anyhow::Context;
use ara_render::scene::{Mesh, Vertex};
use ara_runtime::assets::AssetSource;
use glam::Vec3;
use std::collections::BTreeMap;
use std::path::Path;

/// Project-owned decoded generations. Publication changes invalidate by digest;
/// document transforms and repeated prefab instances do not read model payloads.
pub struct ModelCache {
    reader: ara_play_host::PublishedAssets,
    snapshot: Option<std::sync::Arc<ara_play_host::PublishedSnapshot>>,
    decoded: DecodedModels,
}

impl ModelCache {
    /// Attach once to the project's read-only publication feed; never starts imports.
    pub fn new(root: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            reader: ara_play_host::PublishedAssets::for_project(root)
                .map_err(anyhow::Error::msg)?,
            snapshot: None,
            decoded: Default::default(),
        })
    }

    /// Adopt an available publication and return the revision used by this cache.
    /// Poll before scene synchronization so newly published models replace placeholders.
    /// A busy publisher retains the last snapshot and revision without waiting.
    pub fn revision(&mut self) -> u64 {
        if self
            .snapshot
            .as_ref()
            .is_none_or(|old| old.revision() != self.reader.revision())
        {
            // Pending work and a busy publisher never block the UI. Keep the last
            // immutable snapshot if its replacement is not available this boundary.
            if let Some(snapshot) = self.reader.snapshot() {
                self.snapshot = Some(snapshot);
            }
        }
        self.snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.revision())
    }

    pub(crate) fn load(
        &mut self,
        asset: &str,
        renderer: &ara_runtime::components::MeshRenderer,
        entity: u64,
        name: String,
        material: usize,
    ) -> anyhow::Result<Mesh> {
        self.revision();
        let source = self
            .snapshot
            .as_deref()
            .map(|snapshot| snapshot as &dyn AssetSource);
        mesh(
            self.decoded.get(source, asset)?.map(std::sync::Arc::as_ref),
            renderer,
            entity,
            name,
            material,
        )
    }
}

/// One-shot thumbnail worker entry: the published generation of a source model,
/// with its materials and textures. Scene extraction uses its persistent cache; an
/// isolated preview resolves its sidecar once and waits (bounded, off the UI thread)
/// for an in-flight import to publish.
pub(crate) fn published(
    path: &Path,
    wait: std::time::Duration,
) -> anyhow::Result<std::sync::Arc<ara_runtime::model::Model>> {
    let root = path
        .ancestors()
        .find(|root| root.join("ara.project.json").is_file())
        .context("model has no project root")?;
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(".import.json");
    let value = ara_json::parse(&std::fs::read_to_string(sidecar)?)?;
    let asset = value["id"]
        .as_str()
        .context("model sidecar has no asset ID")?;
    let mut cache = ModelCache::new(root)?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        cache.revision();
        let source = cache
            .snapshot
            .as_deref()
            .map(|snapshot| snapshot as &dyn AssetSource);
        if let Some(model) = cache.decoded.get(source, asset)? {
            return Ok(model.clone());
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "{} has not been imported yet",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[derive(Default)]
struct DecodedModels {
    models: ara_runtime::assets::Models,
    failed: BTreeMap<String, ara_asset::Digest>,
}

impl DecodedModels {
    fn get(
        &mut self,
        source: Option<&dyn AssetSource>,
        asset: &str,
    ) -> anyhow::Result<Option<&std::sync::Arc<ara_runtime::model::Model>>> {
        if let Some(source) = source
            && let Some(digest) = source.digest(asset)
            && self.failed.get(asset) != Some(&digest)
        {
            if let Err(error) = self.models.load(source, asset) {
                self.failed.insert(asset.to_owned(), digest);
                if self.models.get(asset).is_none() {
                    return Err(error.into());
                }
                eprintln!("Scene model {asset} keeps its last good generation: {error}");
            } else {
                self.failed.remove(asset);
            }
        }
        Ok(self.models.get(asset))
    }
}

/// Reads the transactional project default, including unsaved typed settings edits.
pub(crate) fn render_quality(
    workspace: &ara_doc::Workspace,
) -> anyhow::Result<ara_runtime_render::QualityChoice> {
    Ok(render_settings(workspace)?.quality.into())
}

/// Project rendering policy, read without discovering or importing any assets.
pub(crate) fn render_settings(
    workspace: &ara_doc::Workspace,
) -> anyhow::Result<ara_components::render::RenderSettings> {
    Ok(workspace
        .doc("pj_project")
        .and_then(|doc| doc.record("p_settings"))
        .and_then(|record| record.get("settings"))
        .and_then(|settings| settings.get("rendering"))
        .map(serde::Deserialize::deserialize)
        .transpose()?
        .unwrap_or_default())
}

fn mesh(
    model: Option<&ara_runtime::model::Model>,
    renderer: &ara_runtime::components::MeshRenderer,
    entity: u64,
    name: String,
    material: usize,
) -> anyhow::Result<Mesh> {
    let mut mesh = Mesh {
        entity,
        name,
        vertices: Vec::new(),
        indices: Vec::new(),
        material,
    };
    if let Some(model) = model {
        let selection = model.select(renderer.node.as_deref(), renderer.primitive)?;
        let offset = Vec3::from_array(selection.offset);
        for primitive in &model.primitives[selection.primitives] {
            let base = u32::try_from(mesh.vertices.len())?;
            mesh.vertices
                .extend(primitive.vertices.iter().map(|v| Vertex {
                    position: (Vec3::from_array(v.position) + offset).into(),
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
            // The owner can attach/publish after the first Scene frame. Retain
            // the last snapshot while busy; no snapshot means placeholders.
            if let Some(snapshot) = self.reader.snapshot() {
                self.snapshot = Some(snapshot);
            }
        }
        let empty = ara_runtime::assets::MemoryAssets::default();
        let source: &dyn AssetSource = self
            .snapshot
            .as_deref()
            .map_or(&empty as &dyn AssetSource, |snapshot| snapshot);
        // Metadata checks avoid FileCache reads/decodes for unchanged models. The
        // library includes foliage/prefab sources, so extraction reuses them too.
        for (asset, generation) in self
            .snapshot
            .iter()
            .flat_map(|snapshot| snapshot.generations())
        {
            if generation.kinds().contains(&ara_asset::ArtifactKind::Mesh)
                && self
                    .library
                    .models
                    .get(asset)
                    .is_none_or(|model| model.digest != generation.digest())
            {
                self.library.models.load(source, asset)?;
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
        let materials = (
            material_hash.finish(),
            self.snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.revision()),
        );
        if self.materials != Some(materials) {
            let requested: Vec<_> = self.library.materials.ids().map(str::to_owned).collect();
            for asset in requested {
                ara_runtime_render::fidelity::native::when_published(source, |source| {
                    Ok(self.library.materials.load(workspace, source, &asset)?)
                })?;
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
            source,
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
        let empty = ara_runtime::assets::MemoryAssets::default();
        ara_runtime_render::fidelity::native::Content::resolve_available(
            frame,
            workspace,
            schema,
            &self.library,
            self.snapshot
                .as_deref()
                .map_or(&empty as &dyn AssetSource, |snapshot| snapshot),
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn cache_observes_a_publisher_attached_after_placeholder_extraction() -> anyhow::Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-editor/model-publication")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&root)?;
        let mut cache = ModelCache::new(&root)?;
        assert_eq!(cache.revision(), 0);
        let placeholder = cache.load("as_pending", &Default::default(), 1, "Pending".into(), 0)?;
        assert!(!placeholder.vertices.is_empty());
        // No source assets or imports: attaching the control-boundary owner alone
        // publishes the first snapshot, after the viewport already used a placeholder.
        let _owner = ara_play_host::assets::Assets::new(&root, &root.join("absent-worker.exe"))
            .map_err(anyhow::Error::msg)?;
        let revision = cache.revision();
        assert!(revision > 0);
        assert_eq!(
            cache.snapshot.as_ref().context("new snapshot")?.revision(),
            revision
        );
        assert_eq!(cache.revision(), revision);
        Ok(())
    }

    struct Source {
        current: Option<ara_asset::CachedImport>,
        reads: AtomicUsize,
    }
    impl AssetSource for Source {
        fn digest(&self, _: &str) -> Option<ara_asset::Digest> {
            self.current.as_ref().map(|generation| generation.key)
        }
        fn load(
            &self,
            _: &str,
        ) -> ara_runtime::Result<Option<(ara_asset::Digest, ara_asset::CachedImport)>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .current
                .as_ref()
                .map(|generation| (generation.key, generation.clone())))
        }
    }

    fn generation(x: f32) -> ara_asset::CachedImport {
        use ara_asset::{Cancellation, CookPreset, ImportContext, ImportInput, TargetPlatform};
        use serde_json::json;
        let mut scene: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../ara-importers/tests/fixtures/material-extensions.gltf"
        ))
        .expect("fixture");
        scene["nodes"] = json!([
            {"name":"Selected", "mesh":0, "translation":[x,0,0]},
            {"name":"Sibling", "mesh":0, "translation":[20,0,0]}
        ]);
        scene["scenes"] = json!([{"nodes":[0,1]}]);
        let registry = ara_importers::registry().expect("registry");
        let importer = registry.get("ara.gltf").expect("glTF importer");
        let input = ImportInput {
            preset: CookPreset::Fast,
            thread_budget: 1,
            source_path: "cached.gltf".into(),
            source: ara_json::to_canonical_string(&scene).into_bytes(),
            importer: importer.id().into(),
            version: importer.version().into(),
            settings: json!({"virtualGeometry":false}),
            target: TargetPlatform::Windows,
            dependencies: BTreeMap::new(),
            asset_dependencies: BTreeMap::new(),
        };
        let output = registry
            .import(
                &input,
                &mut ImportContext {
                    metrics: None,
                    cancellation: &Cancellation::default(),
                    progress: &mut |_| {},
                },
            )
            .expect("cook");
        ara_asset::CachedImport {
            key: input.key().expect("key"),
            output,
        }
    }

    #[test]
    fn cache_tracks_publication_digests_and_reuses_selected_node_geometry() -> anyhow::Result<()> {
        let mut source = Source {
            current: None,
            reads: AtomicUsize::new(0),
        };
        let mut cache = DecodedModels::default();
        assert!(cache.get(Some(&source), "as_model")?.is_none());
        assert_eq!(source.reads.load(Ordering::Relaxed), 0);
        source.current = Some(generation(7.));
        let first_digest = source
            .digest("as_model")
            .context("published fixture digest")?;
        for _ in 0..100 {
            let model = cache
                .get(Some(&source), "as_model")?
                .context("published model")?;
            assert_eq!(model.digest, first_digest);
        }
        assert_eq!(
            source.reads.load(Ordering::Relaxed),
            1,
            "one payload read/decode for repeated entities and edits"
        );
        let renderer = ara_runtime::components::MeshRenderer {
            node: Some("Selected".into()),
            primitive: Some(0),
            ..Default::default()
        };
        let model = cache
            .get(Some(&source), "as_model")?
            .context("published model")?;
        let selected = mesh(Some(model), &renderer, 42, "Fence".into(), 3)?;
        assert_eq!(selected.indices.len(), model.primitives[0].indices.len());
        assert!(
            selected
                .vertices
                .iter()
                .all(|vertex| vertex.position[0].abs() < 2.),
            "source pivot restored, sibling excluded"
        );
        source.current = Some(generation(9.));
        let changed = cache
            .get(Some(&source), "as_model")?
            .context("changed published model")?
            .digest;
        assert_ne!(changed, first_digest);
        assert_eq!(source.reads.load(Ordering::Relaxed), 2);
        let mut broken = generation(11.);
        broken.output.artifacts.clear();
        source.current = Some(broken);
        for _ in 0..3 {
            assert_eq!(
                cache
                    .get(Some(&source), "as_model")?
                    .context("last good published model")?
                    .digest,
                changed
            );
        }
        assert_eq!(
            source.reads.load(Ordering::Relaxed),
            3,
            "failed generation attempted once, retains last good"
        );
        Ok(())
    }

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
