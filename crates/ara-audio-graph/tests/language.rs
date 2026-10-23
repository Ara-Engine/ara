#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
//! Text, checks, graph projection and typed commands.

use ara_audio_graph::{
    ast::Item,
    commands, dsl,
    library::{self, Op},
    *,
};
use ara_cmd::{Batch, Registry, Session};
use ara_doc::Workspace;
use ara_graph::{DomainAdapter, Editor, Graph};
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::json;

const EXAMPLES: &[&str] = &["footsteps", "wind", "engine", "ui_click", "rifle"];

fn example(name: &str) -> String {
    let path = format!("{}/graphs/{name}.audiograph", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
}

#[test]
fn examples_are_canonical_and_check() {
    for name in EXAMPLES {
        let text = example(name);
        let graph = dsl::parse(&text, "gr_example").unwrap_or_else(|d| panic!("{name}: {d:?}"));
        let printed = dsl::print(&graph);
        assert_eq!(printed, text, "{name} is not in canonical form");
        check(&graph).unwrap_or_else(|d| panic!("{name}: {d:?}"));
        assert_eq!(dsl::parse(&printed, "gr_example").unwrap(), graph);
    }
}

#[test]
fn library_is_documented_and_registers() {
    let registry = library::registry().unwrap();
    for op in Op::ALL {
        let spec = op.spec();
        assert!(!spec.doc.is_empty() && spec.cost > 0, "{}", spec.name);
        assert!(
            spec.inputs.iter().all(|i| !i.doc.is_empty()),
            "{}",
            spec.name
        );
        assert!(registry.get(&op.kind(), 1).is_some());
        assert_eq!(Op::from_name(spec.name), Some(*op));
    }
    assert!(registry.search("granular").next().is_some());
}

fn errors(text: &str) -> Vec<String> {
    match dsl::parse(text, "gr_bad").and_then(|g| check(&g).map(|_| ())) {
        Ok(()) => Vec::new(),
        Err(d) => d.into_iter().map(|d| d.message).collect(),
    }
}

#[test]
fn diagnostics_name_the_problem() {
    let typo = errors("graph A { let x = lowpas(1); output out = x; }");
    assert!(typo[0].contains("did you mean `lowpass`"), "{typo:?}");
    let types = errors("graph A { trigger t; let x = lowpass(t); output out = x; }");
    assert!(
        types[0].contains("input `in` is trigger, expected audio"),
        "{types:?}"
    );
    let missing = errors("graph A { let x = lowpass(); output out = x; }");
    assert!(missing[0].contains("required input `in`"), "{missing:?}");
    let cycle = errors("graph A { let a = add(b, 1); let b = add(a, 1); output out = b; }");
    assert!(cycle.iter().any(|m| m.contains("cycle")), "{cycle:?}");
    let outputs = errors("graph A { let x = sine(); }");
    assert!(outputs[0].contains("output out"), "{outputs:?}");
    let seed = errors("graph A { param s = 1; let x = noise.white(s); output out = x; }");
    assert!(seed[0].contains("number literal"), "{seed:?}");
    let generic = errors("graph A { let x = switch(0, 1, @a.wav); output out = x; }");
    assert!(
        generic[0].contains("earlier items are audio"),
        "{generic:?}"
    );
    let located = dsl::parse("graph A {\n  let x = sine(;\n}", "gr_bad").unwrap_err();
    assert_eq!(located[0].location.line, Some(2), "{}", located[0]);
}

#[test]
fn reconcile_keeps_identity_for_surviving_names() {
    let base = dsl::parse(&example("wind"), "gr_wind").unwrap();
    let edited = example("wind").replace(
        "let body = lowpass(air, 400, 0.7);",
        "let body = lowpass(air, 300, 0.7);\n  let extra = sine(3);",
    );
    let next = dsl::reconcile(&edited, &base).unwrap();
    let id = |g: &AudioGraph, n: &str| g.find(n).unwrap().0.to_owned();
    assert_eq!(id(&base, "body"), id(&next, "body"));
    assert_eq!(next.id, "gr_wind");
    assert!(next.find("extra").is_some());
}

#[test]
fn projection_roundtrips_through_canonical_graph_json() {
    let mut db = SchemaDb::new();
    ara_graph::register(&mut db).unwrap();
    for name in EXAMPLES {
        let graph = dsl::parse(&example(name), "gr_example").unwrap();
        let projected = graph.to_graph(Editor::default()).unwrap();
        let registry = library::registry().unwrap();
        assert!(
            projected.validate(&registry, &AudioAdapter).is_empty(),
            "{name}: {:?}",
            projected.validate(&registry, &AudioAdapter)
        );
        let text = projected.write(&db).unwrap();
        let back = AudioGraph::from_graph(&Graph::read(&text, &db).unwrap()).unwrap();
        assert_eq!(back, graph, "{name}");
        assert!(AudioAdapter.compile(&projected).is_ok());
        let mut moved = projected.clone();
        let first = moved.nodes.keys().next().unwrap().clone();
        moved.editor.positions.insert(first, [10.0, 20.0]);
        assert_eq!(
            moved.semantic_hash(&db).unwrap(),
            projected.semantic_hash(&db).unwrap()
        );
    }
}

fn store(text: &str) -> (Registry, Store) {
    let mut db = SchemaDb::new();
    ara_graph::register(&mut db).unwrap();
    let mut registry = Registry::default();
    commands::register(&mut registry, &mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert!(
        registry.check_examples(&db).is_empty(),
        "{:?}",
        registry.check_examples(&db)
    );
    let graph = dsl::parse(text, "gr_tone").unwrap();
    let mut ws = Workspace::default();
    ws.insert(
        "sounds/tone.graph",
        graph
            .to_graph(Editor::default())
            .unwrap()
            .to_document()
            .unwrap(),
    );
    (registry, Store::new(db, ws))
}

fn current(store: &Store) -> AudioGraph {
    AudioGraph::from_graph(
        &Graph::from_document(store.workspace().doc("gr_tone").unwrap()).unwrap(),
    )
    .unwrap()
}

#[test]
fn commands_edit_by_name_and_undo() {
    let source = "graph Tone {\n  param pitch = 440 range(20, 2000);\n  let osc = sine(pitch);\n  output out = osc;\n}\n";
    let (registry, mut store) = store(source);
    let apply = |store: &mut Store, command: &'static str, params: serde_json::Value| {
        registry.apply(
            store,
            &Batch::new("gr_tone", [(command, params)]),
            &Session::human(),
        )
    };
    let osc_id = current(&store).find("osc").unwrap().0.to_owned();
    apply(
        &mut store,
        "audiograph.rename",
        json!({"from": "osc", "to": "tone"}),
    )
    .unwrap();
    assert_eq!(
        current(&store).find("tone").unwrap().0,
        osc_id,
        "rename keeps identity"
    );
    apply(
        &mut store,
        "audiograph.node.add",
        json!({"name": "soft", "op": "lowpass", "args": {"in": "tone", "cutoff": 800}}),
    )
    .unwrap();
    apply(
        &mut store,
        "audiograph.input.set",
        json!({"node": "output", "input": "out", "value": "soft"}),
    )
    .unwrap();
    apply(
        &mut store,
        "audiograph.param.set",
        json!({"name": "pitch", "default": 220}),
    )
    .unwrap();
    let text = dsl::print(&current(&store));
    assert_eq!(
        text,
        "graph Tone {\n  param pitch = 220 range(20, 2000);\n  let tone = sine(pitch);\n  let soft = lowpass(tone, 800);\n  output out = soft;\n}\n"
    );
    let applied = apply(
        &mut store,
        "audiograph.text.apply",
        json!({"text": text.replace("lowpass(tone, 800)", "highpass(tone, 300)")}),
    )
    .unwrap();
    let after = current(&store);
    assert!(matches!(after.find("soft"), Some((_, Item::Node(n))) if n.op == Op::Highpass));
    assert!(
        applied.outputs[0]
            .to_string()
            .contains("highpass(tone, 300)")
    );
    apply(
        &mut store,
        "audiograph.node.remove",
        json!({"name": "soft"}),
    )
    .unwrap();
    assert!(
        current(&store).outputs.is_empty(),
        "outputs referencing a removed node are cleared"
    );
    for _ in 0..6 {
        store.undo_last(&Author::Human).unwrap();
    }
    assert_eq!(dsl::print(&current(&store)), source);
    assert!(
        apply(
            &mut store,
            "audiograph.rename",
            json!({"from": "nope", "to": "x"})
        )
        .is_err()
    );
    let missing = apply(
        &mut store,
        "audiograph.input.set",
        json!({"node": "osc", "input": "q", "value": 1}),
    );
    assert!(missing.is_err());
    let bad_ref = apply(
        &mut store,
        "audiograph.input.set",
        json!({"node": "osc", "input": "freq", "value": "nothing"}),
    );
    assert!(bad_ref.is_err());
}
