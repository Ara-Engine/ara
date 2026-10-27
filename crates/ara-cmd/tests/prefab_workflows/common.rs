use ara_cmd::{Batch, BatchResult, Registry, Session};
use ara_doc::{AssetId, AssetIndex, IdGen, Workspace, read, write};
use ara_ops::Store;
use ara_schema::SchemaDb;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn setup(records: Value) -> (Store, Registry) {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    let types = ara_json::parse(
        r#"{"$schema":"ara://schema/types/1","types":{
      "Probe":{"kind":"component","doc":"Checks inheritance and typed references.","fields":{
        "value":{"type":"f32","doc":"An inherited scalar."},
        "target":{"type":"entity","doc":"An entity reference."},
        "targets":{"type":"list<entity>","doc":"Nested entity references."},
        "label":{"type":"string","doc":"Text resembling an entity ID."}
      }},"Light":{"kind":"component","doc":"An added component.","fields":{
        "intensity":{"type":"f32","doc":"Scalar added-component value."}
      }}}}"#,
    )
    .unwrap();
    let diagnostics = db.add_json_types(&types);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let registry = Registry::builtin(&mut db);
    assert!(db.verify().is_empty());
    let docs = [
        (
            "base.prefab",
            json!({"$schema":"ara://schema/prefab/1","id":"pf_base","records":{
                "e_base":{"name":"Base","Transform":{},"Probe":{"value":1,"target":"e_child","targets":["e_base","e_child"],"label":"e_child"}},
                "e_child":{"name":"Child","parent":"e_base","order":"a0","Transform":{"position":[1,0,0]},"Probe":{"value":2,"target":"e_base"}}
            }}),
        ),
        (
            "outer.prefab",
            json!({"$schema":"ara://schema/prefab/1","id":"pf_outer","records":{
                "e_outer":{"name":"Outer","Transform":{}},
                "e_nested":{"parent":"e_outer","order":"a0","prefab":"@base.prefab#pf_base"}
            }}),
        ),
        (
            "level.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_level","records":records}),
        ),
    ];
    let mut index = AssetIndex::new();
    for (path, value) in &docs {
        let kind = if path.ends_with(".prefab") {
            "prefab"
        } else {
            "scene"
        };
        index.insert(value["id"].as_str().unwrap(), *path, Some(kind.into()));
    }
    let mut workspace = Workspace::new(index.clone());
    for (path, value) in docs {
        let loaded = read(&value.to_string(), path, &db, Some(&index));
        assert!(
            !loaded.diags.iter().any(|d| d.is_error()),
            "{:#?}",
            loaded.diags
        );
        workspace.insert(path, loaded.doc.unwrap());
    }
    (Store::with_ids(db, workspace, IdGen::seeded(99)), registry)
}

pub fn run(
    store: &mut Store,
    registry: &Registry,
    doc: &str,
    command: &str,
    params: Value,
) -> BatchResult {
    registry
        .apply(
            store,
            &Batch::new(doc, [(command, params)]),
            &Session::human(),
        )
        .unwrap_or_else(|diagnostics| panic!("{command}: {diagnostics:#?}"))
}

pub fn snapshot(store: &Store) -> BTreeMap<String, String> {
    store
        .workspace()
        .docs()
        .map(|(path, doc)| (path.into(), write(doc, store.schema())))
        .collect()
}

pub fn resolved(store: &Store) -> ara_doc::prefab::Resolved {
    let mut diagnostics = Vec::new();
    let entities = store.workspace().resolve(
        &AssetId::parse("sc_level").unwrap(),
        store.schema(),
        &mut diagnostics,
    );
    assert!(
        !diagnostics.iter().any(|d| d.is_error()),
        "{diagnostics:#?}"
    );
    entities
}

pub fn instances() -> Value {
    json!({
        "e_first":{"prefab":"@base.prefab#pf_base","overrides":{"Transform.position":[4,5,6],"Probe.value":7}},
        "e_second":{"prefab":"@base.prefab#pf_base"}
    })
}
