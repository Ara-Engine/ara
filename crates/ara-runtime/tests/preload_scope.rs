//! Runtime construction and live prefab changes prepare only their reachable models.
use ara_doc::{AssetId, Document, Edit, RecordId, Workspace};
use ara_runtime::{Result, Runtime, RuntimeConfig, assets::AssetSource, extract::RenderFrame};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct MissingModels(Arc<AtomicU64>);
impl AssetSource for MissingModels {
    fn load(&self, asset: &str) -> Result<Option<(ara_asset::Digest, ara_asset::CachedImport)>> {
        self.0.fetch_or(
            match asset {
                "as_main" => 1,
                "as_prefab" => 2,
                "as_other" => 4,
                _ => 8,
            },
            Ordering::Relaxed,
        );
        Ok(None)
    }
}

fn add(workspace: &mut Workspace, kind: &str, id: &str, records: Value) {
    let mut document = Document::new(kind, AssetId::parse(id).expect("fixture asset"));
    for (key, value) in records.as_object().expect("records") {
        document.insert_unchecked(
            RecordId::parse(key).expect("record"),
            serde_json::from_value(value.clone()).expect("record object"),
        );
    }
    workspace.insert(format!("{id}.{kind}"), document);
}

#[test]
fn unrelated_models_wait_until_a_prefab_is_referenced_at_a_control_boundary() {
    let mut workspace = Workspace::default();
    add(
        &mut workspace,
        "scene",
        "sc_main",
        json!({
            "e_main":{"MeshRenderer":{"model":"@main.glb#as_main"}},
            "e_instance":{"prefab":"@pf_inner.prefab#pf_inner"}
        }),
    );
    add(
        &mut workspace,
        "prefab",
        "pf_inner",
        json!({"e_root":{"MeshRenderer":{"model":"@prefab.glb#as_prefab"}}}),
    );
    add(
        &mut workspace,
        "prefab",
        "pf_other",
        json!({"e_root":{"MeshRenderer":{"model":"@other.glb#as_other"}}}),
    );
    add(
        &mut workspace,
        "scene",
        "sc_other",
        json!({"e_model":{"MeshRenderer":{"model":"@other.glb#as_other"}}}),
    );
    let reads = Arc::new(AtomicU64::new(0));
    let scene = AssetId::parse("sc_main").expect("scene");
    let mut runtime = Runtime::new(
        ara_runtime::schema(&[]).expect("schema"),
        workspace,
        scene.clone(),
        Box::new(MissingModels(Arc::clone(&reads))),
        RuntimeConfig::default(),
        Vec::new(),
    )
    .expect("playable placeholders");
    assert_eq!(
        reads.load(Ordering::Relaxed),
        3,
        "unrelated prefab/scene must not read payloads"
    );
    assert!(
        !runtime
            .asset_errors()
            .iter()
            .any(|error| error.contains("as_other"))
    );
    let mut frame = RenderFrame::default();
    runtime.extract(&mut frame);
    assert_eq!(
        frame.draws.len(),
        2,
        "root and transitive prefab both project"
    );
    runtime
        .patch_edits(vec![Edit::Set {
            doc: scene,
            id: RecordId::parse("e_instance").expect("instance"),
            path: "prefab".parse().expect("path"),
            value: Some(json!("@pf_other.prefab#pf_other")),
        }])
        .expect("live prefab change");
    assert_eq!(
        reads.load(Ordering::Relaxed),
        7,
        "new prefab model is requested before adoption"
    );
    runtime.step(1, &[]).expect("patched tick");
    runtime.extract(&mut frame);
    assert_eq!(frame.draws.len(), 2);
}
