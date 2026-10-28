//! Selected-root paths and optional policy remain ordinary typed operations.
use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::{Value, json};

fn settings(id: &str, value: Value) -> Document {
    let mut document = Document::new("project", AssetId::parse(id).expect("project ID"));
    document.insert_unchecked(
        RecordId::parse("p_settings").expect("record ID"),
        serde_json::Map::from_iter([("settings".into(), value)]),
    );
    document
}

fn fixture() -> (Registry, Store) {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let registry = Registry::builtin(&mut db);
    let mut workspace = Workspace::default();
    workspace.set_reference_root("selected");
    workspace.insert_project_document(
        "ara.project.json",
        settings("pj_project", json!({"name":"Fixture"})),
    );
    workspace.insert_project_document(
        "settings/project.json",
        settings("pj_preferences", json!({"ui":{"main":"feature/hud.ui"}})),
    );
    workspace.insert(
        "selected/feature/main.scene",
        Document::new("scene", AssetId::parse("sc_main").expect("scene ID")),
    );
    (registry, Store::new(db, workspace))
}

#[test]
fn native_creation_adds_the_selected_root_once_and_marker_uses_short_reference_paths() {
    let (registry, mut store) = fixture();
    registry
        .apply(
            &mut store,
            &Batch::new(
                "pj_project",
                [(
                    "doc.create",
                    json!({"path":"props/box.prefab","id":"pf_box"}),
                )],
            ),
            &Session::human(),
        )
        .expect("create inside selected root");
    assert_eq!(
        store.workspace().path_of("pf_box"),
        Some("selected/props/box.prefab")
    );
    assert_eq!(
        store.workspace().index().path_of("pf_box"),
        Some("props/box.prefab")
    );
    store.undo_last(&Author::Human).expect("undo creation");
    assert!(store.workspace().doc("pf_box").is_none());
    registry
        .apply(
            &mut store,
            &Batch::new(
                "pj_project",
                [(
                    "project.setMain",
                    json!({"scene":"@feature/main.scene#sc_main"}),
                )],
            ),
            &Session::human(),
        )
        .expect("set startup scene");
    assert_eq!(
        store
            .workspace()
            .doc("pj_project")
            .expect("marker")
            .record("p_settings")
            .expect("settings")["settings"]["main"],
        "feature/main.scene"
    );
    let error = registry
        .apply(
            &mut store,
            &Batch::new(
                "pj_preferences",
                [(
                    "project.setMain",
                    json!({"scene":"@feature/main.scene#sc_main"}),
                )],
            ),
            &Session::human(),
        )
        .expect_err("startup marker command cannot silently target optional policy");
    assert!(
        error
            .iter()
            .any(|diagnostic| diagnostic.message.contains("targets ara.project.json"))
    );
}

#[test]
fn optional_policy_nested_values_use_normal_ops_and_undo_without_entering_the_asset_index() {
    let (registry, mut store) = fixture();
    assert!(
        store
            .workspace()
            .index()
            .path_of("pj_preferences")
            .is_none()
    );
    let request = Batch::new(
        "pj_preferences",
        [(
            "property.set",
            json!({"entity":"p_settings","path":"settings.ui.main","value":"other/hud.ui"}),
        )],
    );
    let result = registry
        .apply(&mut store, &request, &Session::human())
        .expect("typed nested policy field");
    assert_eq!(result.ops.len(), 1);
    assert_eq!(
        store
            .workspace()
            .doc("pj_preferences")
            .expect("policy")
            .record("p_settings")
            .expect("settings")["settings"]["ui"]["main"],
        "other/hud.ui"
    );
    store.undo_last(&Author::Human).expect("undo policy edit");
    assert_eq!(
        store
            .workspace()
            .doc("pj_preferences")
            .expect("policy")
            .record("p_settings")
            .expect("settings")["settings"]["ui"]["main"],
        "feature/hud.ui"
    );
    store.redo_last(&Author::Human).expect("redo policy edit");
    let revision = store.revision();
    registry
        .apply(
            &mut store,
            &Batch::new(
                "pj_preferences",
                [(
                    "property.set",
                    json!({"entity":"p_settings","path":"settings","value":false}),
                )],
            ),
            &Session::human(),
        )
        .expect_err("policy stays an object");
    assert_eq!(store.revision(), revision);
    assert!(
        store
            .workspace()
            .index()
            .path_of("pj_preferences")
            .is_none()
    );
}
