#![allow(clippy::unwrap_used)] // Test fixtures may panic on violated expectations.

use std::collections::BTreeMap;

use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, Workspace};
use ara_graph::*;
use ara_ops::{Author, Store};
use ara_schema::SchemaDb;
use serde_json::{Value, json};

fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    register(&mut db).unwrap();
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    db
}
fn node() -> Node {
    Node {
        kind: "test.value".into(),
        version: 1,
        ports: BTreeMap::from([
            (
                "out".into(),
                Port {
                    ty: "float".into(),
                    direction: Direction::Output,
                    required: false,
                    doc: "Scalar output.".into(),
                },
            ),
            (
                "in".into(),
                Port {
                    ty: "float".into(),
                    direction: Direction::Input,
                    required: false,
                    doc: "Optional scalar input.".into(),
                },
            ),
        ]),
        params: BTreeMap::new(),
        payload: json!({"future":{"some":[1,"opaque",true]}}),
        subgraph: None,
    }
}
fn graph() -> Graph {
    let mut g = Graph::new(AssetId::parse("gr_test").unwrap(), "test");
    g.nodes.insert("n_a".into(), node());
    g.nodes.insert("n_b".into(), node());
    g
}
struct Domain {
    cycles: bool,
}
impl DomainAdapter for Domain {
    type Artifact = ();
    fn name(&self) -> &str {
        "test"
    }
    fn allows_cycles(&self) -> bool {
        self.cycles
    }
    fn compile(&self, _: &Graph) -> Result<(), Vec<ara_diag::Diagnostic>> {
        Ok(())
    }
}

#[test]
fn canonical_unknown_payload_and_layout_hash() {
    let db = schema();
    let mut g = graph();
    let before = g.semantic_hash(&db).unwrap();
    g.editor.positions.insert("n_a".into(), [24.0, 48.0]);
    g.editor.comments.insert(
        "c_note".into(),
        Comment {
            text: "Kept apart from semantics.".into(),
            position: [0.0, 2.0],
        },
    );
    g.editor.groups.insert(
        "grp_math".into(),
        Group {
            label: "Math".into(),
            nodes: vec!["n_a".into()],
        },
    );
    g.editor
        .reroutes
        .insert("w_a".into(), vec![[1.0, 2.0], [3.0, 4.0]]);
    assert_eq!(before, g.semantic_hash(&db).unwrap());
    let text = g.write(&db).unwrap();
    let back = Graph::read(&text, &db).unwrap();
    assert_eq!(g, back);
    assert_eq!(text, back.write(&db).unwrap());
    assert!(
        back.validate(&NodeRegistry::default(), &Domain { cycles: false })
            .iter()
            .any(|d| d.code == codes::UNKNOWN_NODE)
    );
    assert_eq!(back.nodes["n_a"].payload, node().payload);
    g.nodes
        .get_mut("n_a")
        .unwrap()
        .params
        .insert("value".into(), json!(0.5));
    assert_ne!(before, g.semantic_hash(&db).unwrap());
    assert!(
        Graph::read(
            &text.replace("\"kind\":", "\"kind\":\"duplicate\",\"kind\":"),
            &db
        )
        .is_err()
    );
}

#[test]
fn domain_rules_and_deterministic_dependency_order() {
    let mut g = graph();
    let mut registry = NodeRegistry::default();
    registry
        .add(NodeKind {
            name: "test.value".into(),
            version: 1,
            doc: "Test numeric expression.".into(),
            ports: node().ports,
            dynamic_ports: false,
        })
        .unwrap();
    let edge = |a: &str, b: &str| Edge {
        from: Endpoint {
            node: a.into(),
            port: "out".into(),
        },
        to: Endpoint {
            node: b.into(),
            port: "in".into(),
        },
    };
    g.edges.insert("w_a".into(), edge("n_a", "n_b"));
    assert_eq!(g.topological_order().unwrap(), ["n_a", "n_b"]);
    assert!(g.validate(&registry, &Domain { cycles: false }).is_empty());
    g.edges.insert("w_b".into(), edge("n_b", "n_a"));
    assert!(
        g.validate(&registry, &Domain { cycles: false })
            .iter()
            .any(|d| d.code == codes::CYCLE)
    );
    assert!(g.validate(&registry, &Domain { cycles: true }).is_empty());
    g.edges.get_mut("w_a").unwrap().to.port = "out".into();
    assert!(
        g.check_wiring(|a, b| a == b, true)
            .iter()
            .any(|d| d.code == codes::PORT)
    );
}

#[test]
fn commands_are_attributed_atomic_undoable_and_layout_only() {
    let mut db = schema();
    let mut reg = Registry::default();
    commands::register(&mut reg, &mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert!(reg.check_examples(&db).is_empty());
    let g = graph();
    let initial = g.write(&db).unwrap();
    let semantic = g.semantic_hash(&db).unwrap();
    let mut ws = Workspace::default();
    ws.insert("graphs/test.graph", g.to_document().unwrap());
    let mut store = Store::new(db, ws);
    let apply = |reg: &Registry, store: &mut Store, command: &'static str, params: Value| {
        reg.apply(
            store,
            &Batch::new("gr_test", [(command, params)]),
            &Session::human(),
        )
    };
    let result = apply(
        &reg,
        &mut store,
        "graph.node.move",
        json!({"node":"n_a","position":[200,100]}),
    )
    .unwrap();
    assert_eq!(result.ops.len(), 1);
    assert_eq!(
        semantic,
        Graph::from_document(store.workspace().doc("gr_test").unwrap())
            .unwrap()
            .semantic_hash(store.schema())
            .unwrap()
    );
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        initial,
        ara_doc::write(store.workspace().doc("gr_test").unwrap(), store.schema())
    );
    for (command, params) in [
        ("graph.node.add", json!({"id":"n_c","node":node()})),
        (
            "graph.param.set",
            json!({"node":"n_c","name":"value","value":2}),
        ),
        (
            "graph.connect",
            json!({"id":"w_a","edge":{"from":{"node":"n_a","port":"out"},"to":{"node":"n_c","port":"in"}}}),
        ),
        ("graph.disconnect", json!({"edge":"w_a"})),
        ("graph.node.remove", json!({"node":"n_c"})),
    ] {
        apply(&reg, &mut store, command, params).unwrap();
    }
    for _ in 0..5 {
        store.undo_last(&Author::Human).unwrap();
    }
    assert_eq!(
        initial,
        ara_doc::write(store.workspace().doc("gr_test").unwrap(), store.schema())
    );
    for _ in 0..5 {
        store.redo_last(&Author::Human).unwrap();
    }
    assert_eq!(
        initial,
        ara_doc::write(store.workspace().doc("gr_test").unwrap(), store.schema())
    );
    let revision = store.revision();
    let bad = Batch::new(
        "gr_test",
        [
            ("graph.node.add", json!({"id":"n_c","node":node()})),
            (
                "graph.connect",
                json!({"edge":{"from":{"node":"n_missing","port":"out"},"to":{"node":"n_c","port":"in"}}}),
            ),
        ],
    );
    assert!(reg.apply(&mut store, &bad, &Session::human()).is_err());
    assert_eq!(revision, store.revision());
    assert!(
        store
            .workspace()
            .doc("gr_test")
            .unwrap()
            .record("n_c")
            .is_none()
    );
}
