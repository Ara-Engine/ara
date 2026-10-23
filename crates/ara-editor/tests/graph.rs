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
