//! The studio headless: its view over a real Editor on a scratch project, driven with
//! pointer input through the stillkit runtime (no window, no renderer).
#![allow(clippy::unwrap_used)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use stillkit::el::Runtime;

use super::{Studio, chrome};
use crate::Editor;

static NEXT: AtomicU64 = AtomicU64::new(0);
const FRAME: Duration = Duration::from_millis(16);

/// A studio over a project with one box at the origin, selected, with the move tool.
fn open() -> Runtime<Studio> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let root = target.join("studio-tests").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
    std::fs::write(root.join("ara.project.json"), ara_json::to_canonical_string(&json!({"layoutVersion": ara_doc::PROJECT_LAYOUT_VERSION, "name": "Studio", "main": "levels/main.scene"}))).unwrap();
    std::fs::write(
        root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
        ara_json::to_canonical_string(
            &json!({"$schema": "ara://schema/scene/1", "id": "sc_studio01", "records": {}}),
        ),
    )
    .unwrap();
    let mut editor = Editor::open(&root).unwrap();
    editor
        .apply(
            vec![
                json!({"cmd":"entity.create","args":{"id":"e_box","name":"Box","components":{"Transform":{"position":[0,0,0]},"MeshRenderer":{"shape":"box"}}}}),
                json!({"cmd":"entity.create","args":{"id":"e_lamp","name":"Lamp","components":{"Transform":{"position":[3,2,0]}}}}),
            ],
            "Create",
            None,
        )
        .unwrap();
    editor
        .action("editor.select", json!({"entities": ["e_box"]}))
        .unwrap();
    editor
        .action(
            "editor.tool",
            json!({"tool": "translate", "snapping": false, "local": false}),
        )
        .unwrap();
    let studio = Studio::new(editor, (1400.0, 900.0));
    let theme = studio.look.theme.clone();
    let mut rt = Runtime::new(studio, chrome::view, theme, (1400, 900), 1.0).unwrap();
    rt.frame(Duration::ZERO);
    rt
}

fn position(rt: &Runtime<Studio>, id: &str) -> Value {
    rt.state().resolved[id].record["Transform"]["position"].clone()
}

/// The window point of the move gizmo's X arrow tip, and the arrow's direction on screen.
fn x_arrow(rt: &Runtime<Studio>) -> ([f32; 2], [f32; 2]) {
    let view = rt.find("scene/view").unwrap();
    let rect = ara_ui::paint::Box2 {
        x: 0.0,
        y: 0.0,
        w: view.width(),
        h: view.height(),
    };
    let h = crate::shell::overlay::handles_in(&rt.state().resolved, &rt.state().editor.state, rect)
        .expect("the selection has a gizmo");
    let along = [h.ends[0][0] - h.origin[0], h.ends[0][1] - h.origin[1]];
    let len = (along[0] * along[0] + along[1] * along[1]).sqrt();
    // Just inside the tip, where the arrow is widest.
    let tip = [h.origin[0] + along[0] * 0.92, h.origin[1] + along[1] * 0.92];
    (
        [view.left + tip[0], view.top + tip[1]],
        [along[0] / len, along[1] / len],
    )
}

#[test]
fn dragging_the_x_arrow_moves_along_x_in_one_undo_step() {
    let mut rt = open();
    let before = rt.state().editor.host.store().revision();
    let (tip, dir) = x_arrow(&rt);
    rt.pointer_move(tip[0], tip[1]);
    rt.frame(FRAME);
    assert!(
        rt.state().local.gizmo_hover.is_some(),
        "the arrow lights under the pointer"
    );
    rt.pointer_down(tip[0], tip[1]);
    for step in 1..=6 {
        let d = step as f32 * 10.0;
        rt.pointer_move(tip[0] + dir[0] * d, tip[1] + dir[1] * d);
        rt.frame(FRAME);
    }
    assert!(
        rt.state().local.gizmo.is_some(),
        "the drag holds the handle"
    );
    rt.pointer_up(tip[0] + dir[0] * 60.0, tip[1] + dir[1] * 60.0);
    rt.frame(FRAME);
    let p = position(&rt, "e_box");
    assert!(p[0].as_f64().unwrap() > 0.1, "moved along +X: {p}");
    assert_eq!(p[1].as_f64().unwrap(), 0.0);
    assert_eq!(p[2].as_f64().unwrap(), 0.0);
    assert_eq!(
        rt.state().editor.host.store().revision(),
        before + 1,
        "one transaction (one revision)"
    );
    assert_eq!(
        rt.state().editor.state.selection,
        vec!["e_box".to_owned()],
        "a drag never re-picks"
    );
}

#[test]
fn escape_cancels_a_gizmo_drag() {
    let mut rt = open();
    let (tip, dir) = x_arrow(&rt);
    rt.pointer_down(tip[0], tip[1]);
    rt.pointer_move(tip[0] + dir[0] * 50.0, tip[1] + dir[1] * 50.0);
    rt.frame(FRAME);
    rt.key(stillkit::el::KeyCode::ESCAPE.0);
    rt.pointer_up(tip[0] + dir[0] * 50.0, tip[1] + dir[1] * 50.0);
    rt.frame(FRAME);
    assert_eq!(position(&rt, "e_box"), json!([0, 0, 0]));
    assert!(rt.state().editor.gesture.is_none());
}

#[test]
fn a_hierarchy_row_click_selects_and_the_inspector_follows() {
    let mut rt = open();
    let row = rt.find("row/e_lamp").unwrap();
    rt.pointer_down(row.center().x, row.center().y);
    rt.pointer_up(row.center().x, row.center().y);
    rt.frame(FRAME);
    assert_eq!(rt.state().editor.state.selection, vec!["e_lamp".to_owned()]);
    assert!(
        rt.find("component/Transform").is_some(),
        "the Inspector shows the lamp's Transform"
    );
    assert!(
        rt.find("component/MeshRenderer").is_none(),
        "and not the box's renderer"
    );
}
