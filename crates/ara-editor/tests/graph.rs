//! Layout gestures remain previews until a single typed edit intent is committed.
use ara_editor::graph::{Canvas, Edit};
use ara_ui::paint::Box2;

#[test]
fn comment_and_group_drags_coalesce_and_escape_keeps_the_source_unchanged() {
    let mut graph = ara_graph::Graph::new(
        ara_doc::AssetId::parse("gr_layout00").expect("fixture ID"),
        "material",
    );
    graph.editor.comments.insert(
        "note".into(),
        ara_graph::Comment {
            text: "Light response".into(),
            position: [400., 450.],
        },
    );
    for (id, position) in [("n_a", [100., 200.]), ("n_b", [380., 200.])] {
        graph.nodes.insert(
            id.into(),
            ara_graph::Node {
                kind: "material.literal".into(),
                version: 1,
                ports: Default::default(),
                params: Default::default(),
                payload: serde_json::json!({"name":id}),
                subgraph: None,
            },
        );
        graph.editor.positions.insert(id.into(), position);
    }
    graph.editor.groups.insert(
        "group".into(),
        ara_graph::Group {
            label: "Surface".into(),
            nodes: vec!["n_a".into(), "n_b".into()],
        },
    );
    let mut text = ara_ui_text::TextSystem::bundled().expect("fonts");
    let mut canvas = Canvas::new(graph, &mut text);
    let rect = Box2 {
        x: 0.,
        y: 0.,
        w: 960.,
        h: 700.,
    };
    let start = canvas.screen([410., 460.], rect);
    canvas.begin(start, 1, false, rect);
    for amount in [10., 20., 50.] {
        canvas.motion([start[0] + amount * canvas.camera.zoom, start[1]]);
    }
    let edits = canvas
        .end([start[0] + 50. * canvas.camera.zoom, start[1]], rect)
        .expect("comment gesture");
    let [Edit::Layout(layout)] = edits.as_slice() else {
        panic!("one layout edit expected");
    };
    assert!((layout.comments["note"].position[0] - 450.).abs() < 0.001);
    assert_eq!(
        canvas.graph().editor.comments["note"].position,
        [400., 450.]
    );
    canvas.begin(start, 1, false, rect);
    canvas.motion([start[0] + 100., start[1]]);
    canvas.cancel();
    assert!(canvas.end(start, rect).expect("cancelled").is_empty());
    let start = canvas.screen([200., 170.], rect);
    canvas.begin(start, 1, false, rect);
    let end = [
        start[0] + 50. * canvas.camera.zoom,
        start[1] + 10. * canvas.camera.zoom,
    ];
    canvas.motion(end);
    let edits = canvas.end(end, rect).expect("group gesture");
    let [Edit::Move(moves)] = edits.as_slice() else {
        panic!("one group movement expected");
    };
    assert_eq!(moves.len(), 2);
    for (id, position) in moves {
        let old = canvas.graph().editor.positions[id];
        assert!((position[0] - old[0] - 50.).abs() < 0.001);
        assert!((position[1] - old[1] - 10.).abs() < 0.001);
    }
}

#[test]
fn text_authored_graphs_lay_out_left_to_right_beside_their_consumers() {
    let mut graph = ara_graph::Graph::new(
        ara_doc::AssetId::parse("gr_layout01").expect("fixture ID"),
        "material",
    );
    let port = |direction| ara_graph::Port {
        ty: "float".into(),
        direction,
        doc: "Test port".into(),
        required: false,
    };
    for (id, inputs) in [("n_a", 0), ("n_b", 1), ("n_c", 0), ("n_output", 2)] {
        let mut ports = std::collections::BTreeMap::from([(
            "out".to_owned(),
            port(ara_graph::Direction::Output),
        )]);
        for i in 0..inputs {
            ports.insert(format!("in{i}"), port(ara_graph::Direction::Input));
        }
        graph.nodes.insert(
            id.into(),
            ara_graph::Node {
                kind: "material.add".into(),
                version: 1,
                ports,
                params: Default::default(),
                payload: serde_json::json!({"name":id,"ty":"float"}),
                subgraph: None,
            },
        );
    }
    for (id, from, to, port) in [
        ("w_ab", "n_a", "n_b", "in0"),
        ("w_bo", "n_b", "n_output", "in0"),
        ("w_co", "n_c", "n_output", "in1"),
    ] {
        graph.edges.insert(
            id.into(),
            ara_graph::Edge {
                from: ara_graph::Endpoint {
                    node: from.into(),
                    port: "out".into(),
                },
                to: ara_graph::Endpoint {
                    node: to.into(),
                    port: port.into(),
                },
            },
        );
    }
    let mut text = ara_ui_text::TextSystem::bundled().expect("fonts");
    let canvas = Canvas::new(graph, &mut text);
    let report = canvas.report(Box2 {
        x: 0.,
        y: 0.,
        w: 1200.,
        h: 800.,
    });
    let boxes = report["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|n| {
            let b = &n["box"];
            let f = |k: &str| b[k].as_f64().expect("box field");
            (
                n["id"].as_str().expect("id").to_owned(),
                [f("x"), f("y"), f("w"), f("h")],
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let x = |id: &str| boxes[id][0];
    assert!(x("n_a") < x("n_b") && x("n_b") < x("n_output"), "{boxes:?}");
    assert_eq!(
        x("n_c"),
        x("n_b"),
        "a constant sits just before its consumer"
    );
    let (b, c) = (boxes["n_b"], boxes["n_c"]);
    assert!(
        b[1] + b[3] <= c[1] || c[1] + c[3] <= b[1],
        "no overlap: {boxes:?}"
    );
}
