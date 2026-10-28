use crate::common::*;
use ara_cmd::builtin::prefab_sources;
use ara_ops::Author;
use serde_json::json;

#[test]
fn create_preserves_pose_root_identity_and_internal_external_references() {
    let (mut store, registry) = setup(json!({
        "e_parent":{"Transform":{"position":[5,0,0]}},
        "e_root":{"name":"Branch","parent":"e_parent","order":"a0","Transform":{"position":[2,3,4],"rotation":[10,20,30],"scale":[2,2,2]},
            "Probe":{"target":"e_child","targets":["e_root","e_external"],"label":"e_child"}},
        "e_child":{"name":"Leaf","parent":"e_root","order":"a0","Transform":{"position":[0,1,0]},"Probe":{"target":"e_root"}},
        "e_external":{"Probe":{"target":"e_child","targets":["e_root","e_child"],"label":"e_child"}}
    }));
    let before = snapshot(&store);
    let result = run(
        &mut store,
        &registry,
        "sc_level",
        "prefab.create",
        json!({
            "entity":"e_root","path":"branch.prefab","id":"pf_branch"
        }),
    );
    assert_eq!(store.history().len(), 1);
    assert_eq!(result.outputs[0]["entity"], "e_root");
    let scene = store.workspace().doc("sc_level").unwrap();
    let instance = scene.record("e_root").unwrap();
    assert_eq!(instance["parent"], "e_parent");
    assert_eq!(instance["order"], "a0");
    assert!(scene.record("e_child").is_none());
    let source = store.workspace().doc("pf_branch").unwrap();
    let source_root = source.children(None)[0].to_string();
    let source_child = source.children(Some(&source_root))[0].to_string();
    assert_ne!(source_root, "e_root");
    assert_ne!(source_child, "e_child");
    assert_eq!(
        source.record(&source_root).unwrap()["Probe"]["target"],
        source_child
    );
    assert_eq!(
        source.record(&source_root).unwrap()["Probe"]["targets"],
        json!([source_root, null])
    );
    assert_eq!(
        source.record(&source_root).unwrap()["Probe"]["label"],
        "e_child"
    );
    assert_eq!(
        source.record(&source_child).unwrap()["Probe"]["target"],
        source_root
    );
    let child_address = format!("e_root/{source_child}");
    let rows = resolved(&store);
    assert_eq!(
        rows["e_root"].record["Transform"]["position"],
        json!([2, 3, 4])
    );
    assert_eq!(
        rows["e_root"].record["Transform"]["rotation"],
        json!([10, 20, 30])
    );
    assert_eq!(
        rows["e_root"].record["Transform"]["scale"],
        json!([2, 2, 2])
    );
    assert_eq!(
        rows["e_root"].record["Probe"]["targets"],
        json!(["e_root", "e_external"])
    );
    assert_eq!(rows["e_root"].record["Probe"]["target"], child_address);
    assert_eq!(rows[&child_address].record["Probe"]["target"], "e_root");
    assert_eq!(rows["e_external"].record["Probe"]["target"], child_address);
    assert_eq!(
        rows["e_external"].record["Probe"]["targets"],
        json!(["e_root", child_address])
    );
    assert_eq!(rows["e_external"].record["Probe"]["label"], "e_child");
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(snapshot(&store), before);
    assert!(store.workspace().doc("pf_branch").is_none());
}

#[test]
fn variant_retains_base_link_removals_overrides_and_follows_later_base_edits() {
    let (mut store, registry) = setup(json!({"e_first":{
        "name":"Variant instance","prefab":"@base.prefab#pf_base",
        "overrides":{"Transform.position":[8,9,10],"Probe.value":11,"Light":{"intensity":6}},
        "removed":["e_child/Probe"]
    },"e_second":{"prefab":"@base.prefab#pf_base"}}));
    let before = snapshot(&store);
    run(
        &mut store,
        &registry,
        "sc_level",
        "prefab.variant",
        json!({
            "entity":"e_first","path":"variant.prefab","id":"pf_variant"
        }),
    );
    let variant = store.workspace().doc("pf_variant").unwrap();
    let root = variant.children(None)[0].to_string();
    let record = variant.record(&root).unwrap();
    assert_eq!(record["prefab"], "@base.prefab#pf_base");
    assert_eq!(record["overrides"]["Probe.value"], 11);
    assert_eq!(record["overrides"]["Light"]["intensity"], 6);
    assert_eq!(record["removed"], json!(["e_child/Probe"]));
    assert!(record["overrides"].get("Transform.position").is_none());
    let sources = prefab_sources(store.workspace(), "sc_level", "e_first").unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| source.doc.as_str())
            .collect::<Vec<_>>(),
        ["pf_variant", "pf_base"]
    );
    assert_eq!(
        resolved(&store)["e_first"].record["Transform"]["position"],
        json!([8, 9, 10])
    );
    assert!(
        !resolved(&store)[&format!("e_first/{root}/e_child")]
            .record
            .contains_key("Probe")
    );
    let created = snapshot(&store);
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(snapshot(&store), before);
    store.redo_last(&Author::Human).unwrap();
    assert_eq!(snapshot(&store), created);
    run(
        &mut store,
        &registry,
        "pf_base",
        "property.set",
        json!({"entity":"e_base","path":"Probe.label","value":"changed base"}),
    );
    let rows = resolved(&store);
    assert_eq!(rows["e_first"].record["Probe"]["label"], "changed base");
    assert_eq!(rows["e_first"].record["Probe"]["value"], 11);
    assert_eq!(rows["e_second"].record["Probe"]["value"], 1);
    run(
        &mut store,
        &registry,
        "sc_level",
        "property.set",
        json!({"entity":"e_first","path":"Probe.value","value":15}),
    );
    run(
        &mut store,
        &registry,
        "sc_level",
        "prefab.apply",
        json!({"entity":"e_first","path":"Probe.value"}),
    );
    assert_eq!(
        store
            .workspace()
            .doc("pf_base")
            .unwrap()
            .record("e_base")
            .unwrap()["Probe"]["value"],
        1
    );
    assert_eq!(
        store
            .workspace()
            .doc("pf_variant")
            .unwrap()
            .record(&root)
            .unwrap()["overrides"]["Probe.value"],
        15
    );
}

#[test]
fn transformless_entities_remain_transformless_when_made_reusable() {
    let (mut store, registry) = setup(json!({"e_data":{"Probe":{"value":3}}}));
    run(
        &mut store,
        &registry,
        "sc_level",
        "prefab.create",
        json!({"entity":"e_data","path":"data.prefab"}),
    );
    assert!(!resolved(&store)["e_data"].record.contains_key("Transform"));
}
