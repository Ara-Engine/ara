use crate::common::*;
use ara_cmd::{Batch, Session};
use ara_ops::Author;
use serde_json::json;

#[test]
fn undo_first_override_restores_absent_container_for_every_authoring_route() {
    for (command, params) in [
        (
            "property.set",
            json!({"entity":"e_first","path":"Probe.value","value":5}),
        ),
        (
            "prefab.override.set",
            json!({"entity":"e_first/e_child","path":"Probe.value","value":5}),
        ),
        (
            "component.add",
            json!({"entity":"e_first","component":"Light","value":{"intensity":5}}),
        ),
    ] {
        let (mut store, registry) = setup(json!({"e_first":{"prefab":"@base.prefab#pf_base"}}));
        let before = snapshot(&store);
        let original = store
            .workspace()
            .doc("sc_level")
            .unwrap()
            .record("e_first")
            .unwrap()
            .clone();
        run(&mut store, &registry, "sc_level", command, params);
        assert!(
            store
                .workspace()
                .doc("sc_level")
                .unwrap()
                .record("e_first")
                .unwrap()
                .contains_key("overrides"),
            "{command}"
        );
        store.undo_last(&Author::Human).unwrap();
        assert_eq!(snapshot(&store), before, "{command}");
        assert_eq!(
            store
                .workspace()
                .doc("sc_level")
                .unwrap()
                .record("e_first")
                .unwrap(),
            &original,
            "{command}"
        );
        store.redo_last(&Author::Human).unwrap();
        assert!(
            store
                .workspace()
                .doc("sc_level")
                .unwrap()
                .record("e_first")
                .unwrap()
                .contains_key("overrides"),
            "{command}"
        );
    }
}

#[test]
fn existing_override_maps_keep_disjoint_participant_edits_independently_undoable() {
    let (mut store, registry) = setup(
        json!({"e_first":{"prefab":"@base.prefab#pf_base","overrides":{"Probe.label":"original"}}}),
    );
    run(
        &mut store,
        &registry,
        "sc_level",
        "property.set",
        json!({"entity":"e_first","path":"Probe.value","value":5}),
    );
    let peer = Session {
        author: Author::Agent("peer".into()),
        scopes: vec!["*".into()],
    };
    registry
        .apply(
            &mut store,
            &Batch::new(
                "sc_level",
                [(
                    "property.set",
                    json!({"entity":"e_first","path":"Probe.label","value":"peer edit"}),
                )],
            ),
            &peer,
        )
        .unwrap();
    store.undo_last(&Author::Human).unwrap();
    let overrides = &store
        .workspace()
        .doc("sc_level")
        .unwrap()
        .record("e_first")
        .unwrap()["overrides"];
    assert_eq!(overrides["Probe.label"], "peer edit");
    assert!(overrides.get("Probe.value").is_none());
    assert_eq!(resolved(&store)["e_first"].record["Probe"]["value"], 1);
}
