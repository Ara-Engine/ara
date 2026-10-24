use super::*;
use ara_asset::{
    CachedImport, Cancellation, CookPreset, ImportContext, ImportInput, TargetPlatform,
};
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_runtime::{
    animation::Library,
    assets::{AssetSource, MemoryAssets},
    extract::TerrainView,
    terrain::{FoliageRule, TerrainData},
};
use ara_schema::AssetRef;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Source {
    entry: CachedImport,
    reads: AtomicUsize,
    forbid: bool,
}
impl AssetSource for Source {
    fn digest(&self, _: &str) -> Option<AssetDigest> {
        Some(self.entry.key)
    }
    fn load(&self, _: &str) -> ara_runtime::Result<Option<(AssetDigest, CachedImport)>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if self.forbid {
            return Err(ara_runtime::RuntimeError(
                "cached prefab must not read a payload".into(),
            ));
        }
        Ok(Some((self.entry.key, self.entry.clone())))
    }
}

#[test]
fn prefab_parts_share_loaded_models_and_reload_a_changed_generation_only_once() {
    let registry = ara_importers::registry().unwrap();
    let importer = registry.get("ara.gltf").unwrap();
    let input = ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 1,
        source_path: "panel.gltf".into(),
        source: include_bytes!(
            "../../../../../ara-importers/tests/fixtures/material-extensions.gltf"
        )
        .to_vec(),
        importer: importer.id().into(),
        version: importer.version().into(),
        settings: serde_json::json!({"virtualGeometry":false}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    };
    let entry = CachedImport {
        key: input.key().unwrap(),
        output: registry
            .import(
                &input,
                &mut ImportContext {
                    metrics: None,
                    cancellation: &Cancellation::default(),
                    progress: &mut |_| {},
                },
            )
            .unwrap(),
    };
    let seed = MemoryAssets(BTreeMap::from([("as_part".into(), entry.clone())]));
    let mut library = Library::default();
    library.models.load(&seed, "as_part").unwrap();
    let known = library.models.get("as_part").unwrap();
    let mut prefab = Document::new("prefab", AssetId::parse("pf_parts").unwrap());
    for (id, value) in [
        (
            "e_root",
            serde_json::json!({"MeshRenderer":{"model":"@panel.gltf#as_part"}}),
        ),
        (
            "e_child",
            serde_json::json!({"parent":"e_root","Transform":{"position":[1,0,0]},"MeshRenderer":{"model":"@panel.gltf#as_part"}}),
        ),
    ] {
        prefab.insert_unchecked(
            RecordId::parse(id).unwrap(),
            serde_json::from_value(value).unwrap(),
        );
    }
    let mut workspace = Workspace::default();
    workspace.insert("parts.prefab", prefab);
    let reference = "@parts.prefab#pf_parts";
    let rule = ara_foliage::Rule {
        name: "parts".into(),
        species: ara_foliage::Species::Rock,
        asset: AssetRef(reference.into()),
        seed: 1,
        cell_size: 4.,
        density: 0.01,
        height: [-10., 10.],
        slope: [0., 90.],
        channel: 0,
        min_weight: 0.,
        noise: 0.,
        scale: [1., 1.],
        enabled: true,
    };
    let mut frame = RenderFrame::default();
    frame.terrain = Some(TerrainView {
        data: Arc::new(TerrainData {
            header: ara_terrain::Header {
                origin: [0.; 2],
                spacing: 1.,
                cells: 16,
                min: ara_terrain::TileCoord { x: 0, z: 0 },
                max: ara_terrain::TileCoord { x: 1, z: 1 },
                materials: Vec::new(),
            },
            tiles: BTreeMap::new(),
            foliage: vec![ara_foliage::Instance {
                position_scale: [0., 0., 0., 1.],
                rotation_wind: [0.; 4],
                identity: [1, 2, 0, 0],
            }],
            foliage_rules: vec![FoliageRule {
                id: "fo_test#r_parts".into(),
                rule,
            }],
        }),
        tiles: Vec::new(),
    });
    let schema = ara_runtime::schema(&[]).unwrap();
    let mut source = Source {
        entry,
        reads: AtomicUsize::new(0),
        forbid: true,
    };
    let content = Content::resolve(&frame, &workspace, &schema, &library, &source).unwrap();
    let parts = &content.foliage_parts[reference];
    assert_eq!(parts.len(), known.primitives.len() * 2);
    assert_eq!(source.reads.load(Ordering::Relaxed), 0);
    for part in parts {
        let Geometry::Model { model, .. } = &part.geometry else {
            panic!("native model part")
        };
        assert!(Arc::ptr_eq(model, known));
    }
    source.forbid = false;
    source.entry.key = AssetDigest::of(b"replacement generation");
    let content = Content::resolve(&frame, &workspace, &schema, &library, &source).unwrap();
    assert_eq!(
        source.reads.load(Ordering::Relaxed),
        1,
        "multiple prefab parts decode only one new generation"
    );
    for part in &content.foliage_parts[reference] {
        let Geometry::Model { model, .. } = &part.geometry else {
            panic!("native model part")
        };
        assert_eq!(model.digest, source.entry.key);
        assert!(!Arc::ptr_eq(model, known));
    }
}
