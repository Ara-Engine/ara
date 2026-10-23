#![allow(clippy::unwrap_used)]
//! Typed behaviour commands: undoable ops, identity preservation, references.
use ara_ai::{ast::Behavior, codes, commands, dsl, projection};
use ara_cmd::{Batch, Registry, Session};
use ara_doc::{Workspace, refs::AssetIndex};
use ara_graph::Graph;
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::{Value, json};

const ID: &str = "gr_guard001";

fn store() -> (Store, Registry) {
    let mut db = SchemaDb::new();
    ara_graph::register(&mut db).unwrap();
    let mut registry = Registry::default();
    commands::register(&mut registry, &mut db);
    ara_graph::commands::register(&mut registry, &mut db);
    let text = std::fs::read_to_string(format!(
        "{}/behaviors/guard.behavior",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let b = dsl::parse(&text, ID).unwrap();
    let mut graph = projection::to_graph(&b, Default::default()).unwrap();
    let chase = b
        .all_states()
        .iter()
        .find(|s| s.name == "Chase")
        .unwrap()
        .id
        .clone();
    graph.editor.positions.insert(chase, [40.0, 120.0]);
    let mut ws = Workspace::new(AssetIndex::new());
    ws.insert("ai/guard.behavior.graph", graph.to_document().unwrap());
    (Store::new(db, ws), registry)
}

fn live(store: &Store) -> (Behavior, Graph) {
    let g = Graph::from_document(store.workspace().doc(ID).unwrap()).unwrap();
    (projection::from_graph(&g).unwrap(), g)
}

fn apply(store: &mut Store, registry: &Registry, command: &'static str, params: Value) -> Value {
    let result = registry
        .apply(
            store,
            &Batch::new(ID, [(command, params)]),
            &Session::human(),
        )
        .unwrap_or_else(|d| panic!("{command}: {d:#?}"));
    result.outputs[0].clone()
}

fn state_id(b: &Behavior, name: &str) -> String {
    b.all_states()
        .iter()
        .find(|s| s.name == name)
        .unwrap()
        .id
        .clone()
}

#[test]
fn text_apply_keeps_identity_and_layout_and_undoes() {
    let (mut store, registry) = store();
    let (before, graph) = live(&store);
    let initial = graph.write(store.schema()).unwrap();
    let chase = state_id(&before, "Chase");
    let text = dsl::print(&before).replace("speed: 3,", "speed: 4.5,");
    let out = apply(
        &mut store,
        &registry,
        "behavior.text.apply",
        json!({ "text": text }),
    );
    assert_eq!(out["diagnostics"], json!([]));
    let (after, graph) = live(&store);
    assert_eq!(state_id(&after, "Chase"), chase);
    assert_eq!(graph.editor.positions[&chase], [40.0, 120.0]);
    assert!(dsl::print(&after).contains("speed: 4.5"));
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(live(&store).1.write(store.schema()).unwrap(), initial);
}

#[test]
fn rename_rewrites_references_and_check_errors_are_reported_not_fatal() {
    let (mut store, registry) = store();
    let (before, _) = live(&store);
    let search = state_id(&before, "Search");
    apply(
        &mut store,
        &registry,
        "behavior.rename",
        json!({"from": "noise", "to": "lastNoise"}),
    );
    apply(
        &mut store,
        &registry,
        "behavior.rename",
        json!({"from": "Search", "to": "Hunt"}),
    );
    let (after, _) = live(&store);
    let text = dsl::print(&after);
    assert!(text.contains("var lastNoise: vec3;"), "{text}");
    assert!(
        text.contains("do moveTo(lastNoise, speed: 2, tolerance: 1.5);"),
        "{text}"
    );
    assert!(text.contains("isSet(lastNoise)"), "{text}");
    assert_eq!(state_id(&after, "Hunt"), search);
    ara_ai::compile(&after).unwrap();
    // Unknown names are kept for repair; the command returns the check errors.
    let broken = text.replace("do wait(2);", "do wait(nap);");
    let out = apply(
        &mut store,
        &registry,
        "behavior.text.apply",
        json!({ "text": broken }),
    );
    let diagnostics: Vec<ara_diag::Diagnostic> =
        serde_json::from_value(out["diagnostics"].clone()).unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == codes::NAME && d.message.contains("`nap`"))
    );
    // Parse errors reject the edit outright.
    let rejected = registry.apply(
        &mut store,
        &Batch::new(ID, [("behavior.text.apply", json!({"text": "behavior {"}))]),
        &Session::human(),
    );
    assert!(rejected.is_err());
    for _ in 0..3 {
        store.undo_last(&Author::Human).unwrap();
    }
    assert_eq!(live(&store).0, before);
}

#[test]
fn structural_commands_add_states_transitions_and_defaults() {
    let (mut store, registry) = store();
    apply(
        &mut store,
        &registry,
        "behavior.state.add",
        json!({"name": "Flee", "parent": "Alert", "when": "lost(target) < 1"}),
    );
    apply(
        &mut store,
        &registry,
        "behavior.transition.add",
        json!({"from": "Flee", "to": "Patrol", "when": "done"}),
    );
    apply(
        &mut store,
        &registry,
        "behavior.var.set",
        json!({"name": "noise", "default": "vec3(1, 0, 2)"}),
    );
    let (b, _) = live(&store);
    let text = dsl::print(&b);
    assert!(
        text.contains("state Flee when lost(target) < 1 {"),
        "{text}"
    );
    assert!(text.contains("on done -> Patrol;"), "{text}");
    assert!(text.contains("var noise: vec3 = vec3(1, 0, 2);"), "{text}");
    apply(
        &mut store,
        &registry,
        "behavior.state.remove",
        json!({"name": "Patrol"}),
    );
    let (b, _) = live(&store);
    assert!(
        !dsl::print(&b).contains("-> Patrol"),
        "transitions into removed states are pruned"
    );
    let duplicate = registry.apply(
        &mut store,
        &Batch::new(ID, [("behavior.state.add", json!({"name": "Chase"}))]),
        &Session::human(),
    );
    assert!(duplicate.is_err());
    for _ in 0..4 {
        store.undo_last(&Author::Human).unwrap();
    }
    assert!(dsl::print(&live(&store).0).contains("state Patrol {"));
}
