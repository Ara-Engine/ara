//! Unchanged pins reuse decoded models without payload I/O; changed pins still reload.
use ara_asset::{
    CachedImport, Cancellation, CookPreset, Digest, ImportContext, ImportInput, TargetPlatform,
};
use ara_runtime::{
    Result, RuntimeError,
    assets::{AssetSource, Models},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Source {
    entry: CachedImport,
    reads: AtomicUsize,
    forbid_payload: bool,
}
impl AssetSource for Source {
    fn digest(&self, _: &str) -> Option<Digest> {
        Some(self.entry.key)
    }
    fn load(&self, _: &str) -> Result<Option<(Digest, CachedImport)>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if self.forbid_payload {
            return Err(RuntimeError(
                "payload read forbidden for unchanged generation".into(),
            ));
        }
        Ok(Some((self.entry.key, self.entry.clone())))
    }
}

#[test]
fn unchanged_metadata_skips_payload_and_changed_digest_reloads_before_publication() {
    let registry = ara_importers::registry().expect("registry");
    let importer = registry.get("ara.gltf").expect("gltf importer");
    let input = ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 1,
        source_path: "panel.gltf".into(),
        source: include_bytes!("../../ara-importers/tests/fixtures/material-extensions.gltf")
            .to_vec(),
        importer: importer.id().into(),
        version: importer.version().into(),
        settings: serde_json::json!({"virtualGeometry":false}),
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
        .expect("fixture cook");
    let mut source = Source {
        entry: CachedImport {
            key: input.key().expect("key"),
            output,
        },
        reads: AtomicUsize::new(0),
        forbid_payload: false,
    };
    let mut models = Models::default();
    assert!(models.load(&source, "as_cached").expect("initial decode"));
    let first = Arc::clone(models.get("as_cached").expect("cached model"));
    source.forbid_payload = true;
    assert!(
        !models
            .load(&source, "as_cached")
            .expect("metadata must prevent payload I/O")
    );
    assert_eq!(source.reads.load(Ordering::Relaxed), 1);
    assert!(Arc::ptr_eq(
        &first,
        models.get("as_cached").expect("unchanged model")
    ));
    source.forbid_payload = false;
    source.entry.key = Digest::of(b"changed generation");
    assert!(
        models
            .load(&source, "as_cached")
            .expect("changed digest decodes")
    );
    assert_eq!(source.reads.load(Ordering::Relaxed), 2);
    let second = Arc::clone(models.get("as_cached").expect("new model"));
    assert_eq!(second.digest, source.entry.key);
    assert!(!Arc::ptr_eq(&first, &second));
    source.entry.key = Digest::of(b"failed generation");
    source.entry.output.artifacts.clear();
    assert!(models.load(&source, "as_cached").is_err());
    assert!(Arc::ptr_eq(
        &second,
        models.get("as_cached").expect("last good model")
    ));
}
