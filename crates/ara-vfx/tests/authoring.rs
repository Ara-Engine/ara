#![allow(clippy::unwrap_used)]
use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Workspace};
use ara_graph::{Editor, Graph};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use ara_vfx::*;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn legacy_widened_f32_graph_values_keep_their_typed_bits() {
    let mut effect = samples::campfire();
    if let Module::Lifetime { seconds, .. } = &mut effect.emitters[0].stacks.initialize[0].module {
        *seconds = 1.4;
    }
    let mut graph = effect
        .to_graph(AssetId::parse("gr_legacy").unwrap(), Editor::default())
        .unwrap();
    for node in graph.nodes.values_mut() {
        let module: Module = serde_json::from_value(node.params["module"].clone()).unwrap();
        node.params
            .insert("module".into(), serde_json::to_value(module).unwrap());
    }
    let read = Effect::from_graph(&graph).unwrap();
    assert_eq!(read, effect);
    let text = read.text().unwrap();
    assert!(text.contains("\"seconds\": 1.4,"));
    assert!(!text.contains("1.399999976"));
}
#[test]
fn samples_roundtrip_text_graph_and_compiled_modules() {
    let mut db = SchemaDb::new();
    ara_graph::register(&mut db).unwrap();
    for (effect, _) in samples::all() {
        let text = effect.text().unwrap();
        let parsed = Effect::parse(&text).unwrap();
        assert_eq!(parsed, effect);
        let graph = effect
            .to_graph(AssetId::parse("gr_test").unwrap(), Editor::default())
            .unwrap();
        let text = graph.write(&db).unwrap();
        let read = Graph::read(&text, &db).unwrap();
        assert_eq!(Effect::from_graph(&read).unwrap(), effect);
        assert!(graph.validate(&registry().unwrap(), &VfxAdapter).is_empty());
        assert!(generate(&effect).unwrap().contains("updateParticle"));
    }
}
#[test]
fn stack_edit_is_atomic_reversible_and_preserves_node_identity() {
    let mut db = SchemaDb::new();
    ara_graph::register(&mut db).unwrap();
    let mut reg = Registry::default();
    commands::register(&mut reg, &mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert!(reg.check_examples(&db).is_empty());
    let graph = samples::campfire()
        .to_graph(AssetId::parse("gr_test").unwrap(), Editor::default())
        .unwrap();
    let before = graph.write(&db).unwrap();
    let mut ws = Workspace::default();
    ws.insert("fx/fire.graph", graph.to_document().unwrap());
    let mut store = Store::new(db, ws);
    let command = (
        "vfx.stack.edit",
        json!({"emitter":"flame","stage":"update","operation":"move","name":"drag","index":0}),
    );
    let dry = reg
        .apply(
            &mut store,
            &Batch {
                dry_run: true,
                ..Batch::new("gr_test", [command.clone()])
            },
            &Session::human(),
        )
        .unwrap();
    assert!(!dry.ops.is_empty());
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc("gr_test").unwrap(), store.schema())
    );
    reg.apply(
        &mut store,
        &Batch::new("gr_test", [command]),
        &Session::human(),
    )
    .unwrap();
    let after = Graph::from_document(store.workspace().doc("gr_test").unwrap()).unwrap();
    assert_eq!(
        graph.nodes.keys().collect::<Vec<_>>(),
        after.nodes.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        Effect::from_graph(&after).unwrap().emitters[0]
            .stacks
            .update[0]
            .name,
        "drag"
    );
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc("gr_test").unwrap(), store.schema())
    );
    store.redo_last(&Author::Human).unwrap();
    let revision = store.revision();
    assert!(reg.apply(&mut store,&Batch::new("gr_test",[("vfx.stack.edit",json!({"emitter":"flame","stage":"initialize","operation":"remove","name":"lifetime"}))]),&Session::human()).is_err());
    assert_eq!(revision, store.revision());
}
#[test]
fn typed_bindings_and_quality_diagnostics() {
    let mut effect = samples::campfire();
    effect.parameters.push(Parameter {
        namespace: Namespace::User,
        name: "rate".into(),
        doc: "Flame particles per second.".into(),
        default: Value::Float(120.),
    });
    effect.emitters[0].stacks.spawn[0]
        .bindings
        .insert("per_second".into(), "user.rate".into());
    let resolved = effect
        .resolve(&BTreeMap::from([("user.rate".into(), Value::Float(60.))]))
        .unwrap();
    assert_eq!(spawn_demand(&resolved.emitters[0], 0, 60), 1);
    assert!(
        effect
            .resolve(&BTreeMap::from([("user.rate".into(), Value::Bool(true))]))
            .is_err()
    );
    assert!(!Module::Rate { per_second: 1. }.accepts_binding("per_second", &Value::Uint(1)));
    assert!(
        !Module::Point { position: [0.; 3] }.accepts_binding("position", &Value::Color([1.; 4]))
    );
    effect.quality.particles = 10;
    assert!(!effect.diagnostics().is_empty());
    let capacities = effect.capacities();
    assert_eq!(capacities[0], 10);
    assert!(capacities[1..].iter().all(|capacity| *capacity == 0));
    let mut bad = samples::sparks();
    bad.emitters[0].stacks.initialize[0].module = Module::Drag { coefficient: 0. };
    assert!(bad.validate().is_err());
    assert!(Effect::parse("{\"name\":\"a\",\"name\":\"b\"}").is_err());
}
#[test]
fn disabled_modules_stay_authored_but_never_run() {
    let mut db = SchemaDb::new();
    ara_graph::register(&mut db).unwrap();
    let mut reg = Registry::default();
    commands::register(&mut reg, &mut db);
    let graph = samples::campfire()
        .to_graph(AssetId::parse("gr_test").unwrap(), Editor::default())
        .unwrap();
    let before = graph.write(&db).unwrap();
    let mut ws = Workspace::default();
    ws.insert("fx/fire.graph", graph.to_document().unwrap());
    let mut store = Store::new(db, ws);
    let disable = (
        "vfx.module.enable",
        json!({"emitter":"flame","stage":"update","name":"curl","enabled":false}),
    );
    reg.apply(
        &mut store,
        &Batch::new("gr_test", [disable]),
        &Session::human(),
    )
    .unwrap();
    let effect = Effect::from_graph(
        &Graph::from_document(store.workspace().doc("gr_test").unwrap()).unwrap(),
    )
    .unwrap();
    // Authored: still in its place, flagged off, and the text says so.
    let update = &effect.emitters[0].stacks.update;
    assert!(update.iter().any(|m| m.name == "curl" && !m.enabled));
    assert!(effect.text().unwrap().contains("\"enabled\""));
    assert!(!samples::campfire().text().unwrap().contains("\"enabled\""));
    // Executed: resolve, code generation and the CPU reference all skip it.
    let resolved = effect.resolve(&BTreeMap::new()).unwrap();
    assert!(
        !resolved.emitters[0]
            .stacks
            .update
            .iter()
            .any(|m| m.name == "curl")
    );
    assert_ne!(
        generate(&effect).unwrap(),
        generate(&samples::campfire()).unwrap()
    );
    CpuSystem::new(&effect).unwrap().step(None).unwrap();
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc("gr_test").unwrap(), store.schema())
    );
}
