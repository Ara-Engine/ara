//! Typed entity references track root collapse and every nested prefab boundary.
#![allow(clippy::unwrap_used)]
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_schema::SchemaDb;
use serde_json::{Value, json};

fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    let types = ara_json::parse(
        r#"{"$schema":"ara://schema/types/1","types":{
      "Links":{"kind":"component","doc":"Typed reference containers.","fields":{
        "target":{"type":"entity","doc":"One entity."},
        "optional":{"type":"option<entity>","doc":"Optional entity."},
        "list":{"type":"list<entity>","doc":"Entity list."},
        "keyed":{"type":"keyed<Slot>","doc":"Keyed entity slots."},
        "map":{"type":"map<entity>","doc":"Named entities."},
        "label":{"type":"string","doc":"Unrelated text."},
        "asset":{"type":"asset","doc":"Unrelated asset."}
      }},"Slot":{"kind":"struct","doc":"A stable slot.","fields":{
        "id":{"type":"string","doc":"Stable identity."},
        "target":{"type":"entity","doc":"Slot entity."}
      }}}}"#,
    )
    .unwrap();
    assert!(db.add_json_types(&types).is_empty());
    assert!(db.verify().is_empty());
    db
}

fn document(kind: &str, id: &str, records: Value) -> Document {
    let mut doc = Document::new(kind, AssetId::parse(id).unwrap());
    for (id, record) in records.as_object().unwrap() {
        doc.insert_unchecked(
            RecordId::parse(id).unwrap(),
            record.as_object().unwrap().clone(),
        );
    }
    doc
}

#[test]
fn nested_variants_rewrite_only_typed_references_and_keep_scene_overrides() {
    let db = schema();
    let mut workspace = Workspace::default();
    workspace.insert("base.prefab", document("prefab","pf_base",json!({
        "e_base":{"Links":{"target":"e_child","optional":"e_base","list":["e_child","e_base",null],
            "keyed":[{"id":"slot1","target":"e_child"},{"id":"slot2","target":"e_base"}],
            "map":{"child":"e_child","root":"e_base"},"label":"e_child","asset":"@asset.prefab#pf_base"}},
        "e_child":{"parent":"e_base","Links":{"target":"e_base"}}
    })));
    workspace.insert(
        "outer.prefab",
        document(
            "prefab",
            "pf_outer",
            json!({
                "e_outer":{"Links":{"target":"e_nested/e_child"}},
                "e_nested":{"parent":"e_outer","prefab":"@base.prefab#pf_base"}
            }),
        ),
    );
    workspace.insert("variant.prefab", document("prefab","pf_variant",json!({
        "e_variant":{"prefab":"@outer.prefab#pf_outer","overrides":{"e_nested/Links.target":"e_variant/e_nested/e_child"}}
    })));
    workspace.insert("scene.scene", document("scene","sc_scene",json!({
        "e_one":{"prefab":"@variant.prefab#pf_variant","overrides":{"e_variant/e_nested/Links.target":"e_external"}},
        "e_two":{"prefab":"@variant.prefab#pf_variant"},
        "e_direct":{"prefab":"@base.prefab#pf_base"},
        "e_external":{"Links":{"target":"e_one/e_variant/e_nested/e_child"}}
    })));
    let mut diagnostics = Vec::new();
    let rows = workspace.resolve(&AssetId::parse("sc_scene").unwrap(), &db, &mut diagnostics);
    assert!(
        !diagnostics.iter().any(|d| d.is_error()),
        "{diagnostics:#?}"
    );
    let prefix = "e_two/e_variant/e_nested";
    let child = format!("{prefix}/e_child");
    let links = &rows[prefix].record["Links"];
    assert_eq!(links["target"], child);
    assert_eq!(links["optional"], prefix);
    assert_eq!(links["list"], json!([child, prefix, null]));
    assert_eq!(
        links["keyed"],
        json!([{"id":"slot1","target":child},{"id":"slot2","target":prefix}])
    );
    assert_eq!(links["map"], json!({"child":child,"root":prefix}));
    assert_eq!(links["label"], "e_child");
    assert_eq!(links["asset"], "@asset.prefab#pf_base");
    assert_eq!(rows[&child].record["Links"]["target"], prefix);
    assert_eq!(
        rows["e_one/e_variant/e_nested"].record["Links"]["target"],
        "e_external"
    );
    assert_eq!(rows["e_direct"].record["Links"]["optional"], "e_direct");
    assert_eq!(
        rows["e_direct"].record["Links"]["target"],
        "e_direct/e_child"
    );
    assert_eq!(
        rows["e_external"].record["Links"]["target"],
        "e_one/e_variant/e_nested/e_child"
    );
}
