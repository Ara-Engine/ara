use crate::common::*;
use ara_cmd::{Batch, Session, builtin::prefab_sources};
use ara_ops::Author;
use serde_json::json;

#[test]
fn apply_changes_source_and_clears_only_authored_values_in_one_undo() {
    let (mut store, registry) = setup(instances());
    let before = snapshot(&store);
    let applied = run(
        &mut store,
        &registry,
        "sc_level",
        "prefab.apply",
        json!({"entity":"e_first"}),
    );
    assert_eq!(store.history().len(), 1);
    assert_eq!(store.history()[0].ops, applied.ops);
    assert!(applied.ops.iter().any(|op| op.doc().as_str() == "pf_base"));
    assert!(applied.ops.iter().any(|op| op.doc().as_str() == "sc_level"));
    let rows = resolved(&store);
    assert_eq!(rows["e_first"].record["Probe"]["value"], 7);
    assert_eq!(rows["e_second"].record["Probe"]["value"], 7);
    let overrides = &store
        .workspace()
        .doc("sc_level")
        .unwrap()
        .record("e_first")
        .unwrap()["overrides"];
    assert_eq!(overrides["Transform.position"], json!([4, 5, 6]));
    assert!(overrides.get("Probe.value").is_none());
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(snapshot(&store), before);
    store.redo_last(&Author::Human).unwrap();
    assert_eq!(resolved(&store)["e_second"].record["Probe"]["value"], 7);
}

#[test]
fn nested_source_choice_preserves_outer_boundary_or_explicitly_edits_base() {
    let (mut store, registry) = setup(
        json!({"e_first":{"prefab":"@outer.prefab#pf_outer","overrides":{
            "e_nested/e_child/Probe.value":9
        }}}),
    );
    let before_nearest = snapshot(&store);
    let original_outer = store
        .workspace()
        .doc("pf_outer")
        .unwrap()
        .record("e_nested")
        .unwrap()
        .clone();
    let sources =
        prefab_sources(store.workspace(), "sc_level", "e_first/e_nested/e_child").unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| (
                source.doc.as_str(),
                source.entity.as_str(),
                source.instance.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            ("pf_outer", "e_nested/e_child", "e_first"),
            ("pf_base", "e_child", "e_first/e_nested")
        ]
    );
    run(
        &mut store,
        &registry,
        "sc_level",
        "prefab.apply",
        json!({"entity":"e_first/e_nested/e_child"}),
    );
    assert_eq!(
        store
            .workspace()
            .doc("pf_base")
            .unwrap()
            .record("e_child")
            .unwrap()["Probe"]["value"],
        2
    );
    assert_eq!(
        store
            .workspace()
            .doc("pf_outer")
            .unwrap()
            .record("e_nested")
            .unwrap()["overrides"]["e_child/Probe.value"],
        9
    );
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(snapshot(&store), before_nearest);
    assert_eq!(
        store
            .workspace()
            .doc("pf_outer")
            .unwrap()
            .record("e_nested")
            .unwrap(),
        &original_outer,
        "Undo must restore an absent override container, not leave an empty map"
    );
    run(
        &mut store,
        &registry,
        "sc_level",
        "prefab.apply",
        json!({
            "entity":"e_first/e_nested/e_child","destination":"@base.prefab#pf_base"
        }),
    );
    assert_eq!(
        store
            .workspace()
            .doc("pf_base")
            .unwrap()
            .record("e_child")
            .unwrap()["Probe"]["value"],
        9
    );
    assert!(
        store
            .workspace()
            .doc("pf_outer")
            .unwrap()
            .record("e_nested")
            .unwrap()
            .get("overrides")
            .is_none()
    );
}

#[test]
fn added_components_and_removed_children_share_the_apply_transaction() {
    let (mut store, registry) = setup(
        json!({"e_first":{"prefab":"@base.prefab#pf_base","overrides":{
        "Light":{"intensity":5}
    },"removed":["e_child"]},"e_second":{"prefab":"@base.prefab#pf_base"}}),
    );
    let before = snapshot(&store);
    run(
        &mut store,
        &registry,
        "sc_level",
        "prefab.apply",
        json!({"entity":"e_first"}),
    );
    assert!(
        store
            .workspace()
            .doc("pf_base")
            .unwrap()
            .record("e_child")
            .is_none()
    );
    assert_eq!(resolved(&store)["e_second"].record["Light"]["intensity"], 5);
    assert!(
        store
            .workspace()
            .doc("sc_level")
            .unwrap()
            .record("e_first")
            .unwrap()
            .get("removed")
            .is_none()
    );
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(snapshot(&store), before);
}

#[test]
fn source_references_are_local_and_another_instance_is_rejected_atomically() {
    let (mut store, registry) = setup(instances());
    run(
        &mut store,
        &registry,
        "sc_level",
        "property.set",
        json!({
            "entity":"e_first","path":"Probe.target","value":"e_first/e_child"
        }),
    );
    run(
        &mut store,
        &registry,
        "sc_level",
        "prefab.apply",
        json!({"entity":"e_first","path":"Probe.target"}),
    );
    assert_eq!(
        store
            .workspace()
            .doc("pf_base")
            .unwrap()
            .record("e_base")
            .unwrap()["Probe"]["target"],
        "e_child"
    );
    assert_eq!(
        resolved(&store)["e_second"].record["Probe"]["target"],
        "e_second/e_child"
    );
    run(
        &mut store,
        &registry,
        "sc_level",
        "property.set",
        json!({
            "entity":"e_first","path":"Probe.target","value":"e_second/e_child"
        }),
    );
    let before = snapshot(&store);
    let revision = store.revision();
    let failed = registry
        .apply(
            &mut store,
            &Batch::new("sc_level", [("prefab.apply", json!({"entity":"e_first"}))]),
            &Session::human(),
        )
        .unwrap_err();
    assert!(
        failed
            .iter()
            .any(|d| d.message.contains("outside the chosen prefab instance"))
    );
    assert_eq!(snapshot(&store), before);
    assert_eq!(store.revision(), revision);
}
