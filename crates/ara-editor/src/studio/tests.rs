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
    open_sized((1400, 900))
}

/// [`open`] in a window of `size` (logical px at scale 1).
fn open_sized(size: (u32, u32)) -> Runtime<Studio> {
    let studio = open_studio(size);
    let theme = studio.look.theme.clone();
    let mut rt = Runtime::new(studio, chrome::view, theme, size, 1.0).unwrap();
    rt.frame(Duration::ZERO);
    rt
}

/// The studio [`open_sized`] shows: a project of four entities, one selected.
fn open_studio(size: (u32, u32)) -> Studio {
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
    // Each studio test keeps its own user settings, so rebinding or reordering in one
    // never reaches another.
    let mut editor = Editor::open_with(&root, Some(root.join(".ara/test-user.json"))).unwrap();
    editor
        .apply(
            vec![
                json!({"cmd":"entity.create","args":{"id":"e_box","name":"Box","components":{"Transform":{"position":[0,0,0]},"MeshRenderer":{"shape":"box"}}}}),
                json!({"cmd":"entity.create","args":{"id":"e_lamp","name":"Lamp","components":{"Transform":{"position":[3,2,0]}}}}),
                json!({"cmd":"entity.create","args":{"id":"e_sun","name":"Sun","components":{"Transform":{},"Light":{"kind":"directional"}}}}),
                json!({"cmd":"entity.create","args":{"id":"e_post","name":"Post","components":{"Transform":{},"PostProcessVolume":{"shape":"global"}}}}),
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
    Studio::new(editor, (size.0 as f32, size.1 as f32))
}

/// A few frames, applying focus requests as the host does.
fn settle(rt: &mut Runtime<Studio>) {
    for _ in 0..3 {
        rt.frame(FRAME);
        if let Some(key) = rt.touch(|s| s.local.focus.take()) {
            rt.focus(key);
        }
    }
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
    let h = crate::overlay::handles_in(&rt.state().resolved, &rt.state().editor.state, rect)
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
fn a_move_with_surface_or_vertex_snapping_asks_the_host_instead_of_stepping() {
    let mut rt = open();
    rt.update(|s| {
        s.act(
            "editor.settings.set",
            json!({"key": "viewport.surfaceSnap", "value": true}),
        )
    });
    settle(&mut rt);
    let (tip, dir) = x_arrow(&rt);
    rt.pointer_move(tip[0], tip[1]);
    rt.frame(FRAME);
    rt.pointer_down(tip[0], tip[1]);
    rt.pointer_move(tip[0] + dir[0] * 40.0, tip[1] + dir[1] * 40.0);
    rt.frame(FRAME);
    let request = rt
        .state()
        .local
        .snap
        .expect("the host is asked to snap the move");
    assert!(!request.vertex, "onto a surface");
    // Held V makes it vertex to vertex.
    rt.touch(|s| s.local.vertex_key = true);
    rt.pointer_move(tip[0] + dir[0] * 50.0, tip[1] + dir[1] * 50.0);
    rt.frame(FRAME);
    assert!(rt.state().local.snap.is_some_and(|r| r.vertex));
    // Until the Scene presents this scene, the host has no geometry to snap to.
    let scene = crate::studio::viewport::SceneView::default();
    let view = rt.state().editor.view().unwrap();
    let capture =
        super::snapping::Snap::capture(&rt.state().editor, &view, &scene, &|_| None, &request);
    assert!(capture.is_none(), "it waits for presented geometry");
    rt.pointer_up(tip[0] + dir[0] * 50.0, tip[1] + dir[1] * 50.0);
    rt.frame(FRAME);
    assert_eq!(
        position(&rt, "e_box")[0].as_f64().unwrap(),
        0.0,
        "no grid move meanwhile"
    );
}

#[test]
fn a_value_that_differs_across_the_selection_shows_a_dash_and_takes_one_for_all() {
    let mut rt = open();
    // A second box, rougher than the first.
    rt.update(|s| {
        s.apply(
            vec![json!({"cmd": "entity.create", "args": {"id": "e_crate", "name": "Crate", "components": {"Transform": {"position": [2, 0, 0]}, "MeshRenderer": {"shape": "box", "roughness": 0.9}}}})],
            "Add a crate",
        );
        s.select(vec!["e_box".into(), "e_crate".into()], false);
    });
    settle(&mut rt);
    let key = "field/MeshRenderer.roughness";
    let shown = rt
        .tree()
        .into_iter()
        .find(|n| n.key.as_ref().is_some_and(|k| k.to_string() == key));
    let shown = shown.expect("the field shows");
    assert_eq!(
        shown.value.or(shown.text).as_deref(),
        Some("—"),
        "shows a dash"
    );
    let r = rt.find(key).expect("the field");
    rt.pointer_move(r.center().x, r.center().y);
    rt.frame(FRAME);
    rt.pointer_down(r.center().x, r.center().y);
    rt.pointer_up(r.center().x, r.center().y);
    rt.frame(FRAME);
    rt.type_text("0.25");
    rt.key(0x0D);
    settle(&mut rt);
    for id in ["e_box", "e_crate"] {
        let v = &rt.state().resolved[id].record["MeshRenderer"]["roughness"];
        assert_eq!(v.as_f64(), Some(0.25), "{id} takes it: {v}");
    }
}

#[test]
fn the_tools_overlay_selects_without_a_gizmo_and_moves_with_one() {
    let mut rt = open();
    settle(&mut rt);
    press(&mut rt, "scene/view.tool.select");
    assert!(rt.state().editor.state.hide_gizmos, "Select shows no gizmo");
    press(&mut rt, "scene/view.tool.move");
    assert!(!rt.state().editor.state.hide_gizmos, "Move brings it back");
}

#[test]
fn two_d_mode_looks_head_on_and_flat_and_right_drags_pan() {
    use crate::state::CameraProjection;
    let mut rt = open();
    settle(&mut rt);
    press(&mut rt, "scene/2d");
    let camera = rt.state().editor.state.camera;
    assert_eq!(camera.projection, CameraProjection::Orthographic);
    assert_eq!((camera.yaw, camera.pitch), (0.0, 0.0), "head-on");
    // A right drag pans; the view doesn't turn.
    let view = rt.find("scene/view").unwrap();
    let (x, y) = (view.center().x, view.center().y);
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    rt.handle(&stillkit::Event::Press(
        stillkit::Pointer {
            at: stillkit::Point::new(x, y),
            screen: (0, 0),
        },
        stillkit::Button::Right,
    ));
    rt.handle(&stillkit::Event::Move(stillkit::Pointer {
        at: stillkit::Point::new(x + 60.0, y),
        screen: (0, 0),
    }));
    rt.frame(FRAME);
    rt.handle(&stillkit::Event::Release(
        stillkit::Pointer {
            at: stillkit::Point::new(x + 60.0, y),
            screen: (0, 0),
        },
        stillkit::Button::Right,
    ));
    settle(&mut rt);
    let panned = rt.state().editor.state.camera;
    assert_eq!((panned.yaw, panned.pitch), (0.0, 0.0), "still head-on");
    assert_ne!(panned.target, camera.target, "panned");
    // Off again: perspective, at an angle.
    press(&mut rt, "scene/2d");
    assert_eq!(
        rt.state().editor.state.camera.projection,
        CameraProjection::Perspective
    );
}

#[test]
fn a_vector_that_differs_shows_a_dash_per_axis_and_each_keeps_its_others() {
    let mut rt = open();
    rt.update(|s| s.select(vec!["e_box".into(), "e_lamp".into()], false));
    settle(&mut rt);
    let shown = |rt: &Runtime<Studio>, axis: usize| {
        let key = format!("field/Transform.position/{axis}");
        rt.tree()
            .into_iter()
            .find(|n| n.key.as_ref().is_some_and(|k| k.to_string() == key))
            .and_then(|n| n.value.or(n.text))
            .map(|t| t.to_string())
    };
    assert_eq!(shown(&rt, 0).as_deref(), Some("—"), "X differs (0, 3)");
    assert_eq!(shown(&rt, 1).as_deref(), Some("—"), "Y differs (0, 2)");
    assert_eq!(shown(&rt, 2).as_deref(), Some("0"), "Z is shared");
    let r = rt.find("field/Transform.position/0").expect("X");
    rt.pointer_move(r.center().x, r.center().y);
    rt.frame(FRAME);
    rt.pointer_down(r.center().x, r.center().y);
    rt.pointer_up(r.center().x, r.center().y);
    rt.frame(FRAME);
    rt.type_text("5");
    rt.key(0x0D);
    settle(&mut rt);
    let position = |id: &str| rt.state().resolved[id].record["Transform"]["position"].clone();
    assert_eq!(position("e_box"), json!([5.0, 0, 0]));
    assert_eq!(
        position("e_lamp"),
        json!([5.0, 2, 0]),
        "the Lamp keeps its Y"
    );
}

/// A right press at `from`, moved through `path`, released at its last point.
fn right_drag(rt: &mut Runtime<Studio>, from: (f32, f32), path: &[(f32, f32)]) {
    let pointer = |(x, y): (f32, f32)| stillkit::Pointer {
        at: stillkit::Point::new(x, y),
        screen: (0, 0),
    };
    rt.pointer_move(from.0, from.1);
    rt.frame(FRAME);
    rt.handle(&stillkit::Event::Press(
        pointer(from),
        stillkit::Button::Right,
    ));
    for at in path {
        rt.handle(&stillkit::Event::Move(pointer(*at)));
        rt.frame(FRAME);
    }
    let end = path.last().copied().unwrap_or(from);
    rt.handle(&stillkit::Event::Release(
        pointer(end),
        stillkit::Button::Right,
    ));
    settle(rt);
}

#[test]
fn a_right_drag_that_comes_back_is_not_a_right_click() {
    let mut rt = open();
    settle(&mut rt);
    let view = rt.find("scene/view").unwrap();
    let (x, y) = (view.center().x, view.center().y);
    // Look around and come back to where it began: no menu.
    // (The host opens the menu from the pick a right click leaves.)
    let asks_for_menu = |rt: &Runtime<Studio>| {
        rt.state()
            .local
            .pick
            .as_ref()
            .is_some_and(|p| p.context.is_some())
    };
    right_drag(
        &mut rt,
        (x, y),
        &[(x + 30.0, y), (x + 60.0, y + 20.0), (x + 30.0, y), (x, y)],
    );
    assert!(!asks_for_menu(&rt), "a drag opens no menu");
    // A still right click does.
    right_drag(&mut rt, (x, y), &[]);
    assert!(asks_for_menu(&rt), "a click opens the menu");
    // The Material canvas pans on a right drag and searches nodes on a right click.
    rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/drag.graph", "domain": "material"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
    });
    rt.update(|s| super::commands::run(s, "window.material", &crate::catalog::Target::None));
    rt.update(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    let canvas = rt.find("graph/view").expect("the canvas");
    let (x, y) = (canvas.center().x, canvas.center().y);
    right_drag(&mut rt, (x, y), &[(x + 40.0, y + 10.0), (x, y)]);
    assert!(rt.state().graph.adding.is_none(), "a pan opens no search");
    right_drag(&mut rt, (x, y), &[]);
    assert!(
        rt.state().graph.adding.is_some(),
        "a click opens the search"
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
fn a_reference_field_searches_and_picks_an_entity() {
    let mut rt = open();
    rt.update(|s| {
        s.apply(
            vec![json!({"cmd":"entity.create","args":{"id":"e_rig","name":"Rig","components":{"Transform":{},"FollowCamera":{}}}})],
            "Create rig",
        );
        s.act("editor.select", json!({"entities": ["e_rig"]}));
    });
    settle(&mut rt);
    let field = rt
        .find("field/FollowCamera.target")
        .expect("the target is an object field");
    rt.pointer_move(field.center().x, field.center().y);
    rt.frame(FRAME);
    rt.pointer_down(field.center().x, field.center().y);
    rt.pointer_up(field.center().x, field.center().y);
    settle(&mut rt);
    assert!(rt.find("reference/panel").is_some(), "the picker opens");
    rt.type_text("lam");
    settle(&mut rt);
    let item = rt.find("reference/item/1").expect("Lamp is found");
    assert!(rt.find("reference/item/2").is_none(), "and only Lamp");
    rt.pointer_move(item.center().x, item.center().y);
    rt.frame(FRAME);
    rt.pointer_down(item.center().x, item.center().y);
    rt.pointer_up(item.center().x, item.center().y);
    settle(&mut rt);
    assert!(rt.state().local.reference.is_none(), "picking closes it");
    assert_eq!(
        rt.state().resolved["e_rig"].record["FollowCamera"]["target"],
        json!("e_lamp")
    );
}

#[test]
fn a_list_field_adds_edits_and_removes_elements_in_place() {
    let mut rt = open();
    settle(&mut rt);
    let overrides = |rt: &Runtime<Studio>| {
        rt.state().resolved["e_box"].record["MeshRenderer"]["materialOverrides"].clone()
    };
    press(&mut rt, "compound/MeshRenderer.materialOverrides/add");
    assert_eq!(
        overrides(&rt),
        json!([{"primitive": 0, "material": null}]),
        "Add appends a default element"
    );
    assert!(
        rt.find("compound/MeshRenderer.materialOverrides#/0")
            .is_some(),
        "and unfolds the list"
    );
    let field = "compound/MeshRenderer.materialOverrides#/0/primitive/value";
    press(&mut rt, field);
    rt.type_text("2");
    rt.key(0x0D);
    settle(&mut rt);
    assert_eq!(
        overrides(&rt)[0]["primitive"],
        json!(2),
        "a nested number types in"
    );
    assert!(
        rt.find("field/MeshRenderer.materialOverrides#/0/material")
            .is_some(),
        "a nested asset is an object field"
    );
    press(&mut rt, "compound/MeshRenderer.materialOverrides#/0/remove");
    assert_eq!(overrides(&rt), json!([]), "remove takes it out");
}

#[test]
fn scene_overlays_move_to_another_edge_collapse_and_hide() {
    let mut rt = open();
    settle(&mut rt);
    let dock = |rt: &Runtime<Studio>, id: &str| super::scene_overlays::dock(rt.state(), id);
    track(&mut rt, &["scene/view", "scene/overlay/tools"]);
    let grip = rt
        .find("scene/overlay/tools/grip")
        .expect("the Tools overlay has a grip");
    let view = rt.find("scene/view").expect("the Scene view");
    // Drag the Tools overlay to the bottom right corner.
    rt.pointer_move(grip.center().x, grip.center().y);
    rt.frame(FRAME);
    rt.pointer_down(grip.center().x, grip.center().y);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        rt.pointer_move(
            grip.center().x + (view.right - 60.0 - grip.center().x) * t,
            grip.center().y + (view.bottom - 20.0 - grip.center().y) * t,
        );
        rt.frame(FRAME);
    }
    assert!(
        rt.state().local.overlay_drag.is_some(),
        "the overlay follows the pointer"
    );
    rt.pointer_up(view.right - 60.0, view.bottom - 20.0);
    settle(&mut rt);
    let d = dock(&rt, "tools");
    assert_eq!(
        (d.edge, d.end),
        (super::scene_overlays::Edge::Bottom, true),
        "{d:?}"
    );
    // A click on the grip collapses it to its icon.
    let grip = rt.find("scene/overlay/tools/grip").expect("still there");
    rt.pointer_move(grip.center().x, grip.center().y);
    rt.frame(FRAME);
    rt.pointer_down(grip.center().x, grip.center().y);
    rt.pointer_up(grip.center().x, grip.center().y);
    settle(&mut rt);
    assert!(dock(&rt, "tools").collapsed);
    assert!(
        rt.find("scene/view.tool.move").is_none(),
        "only its icon shows"
    );
    assert!(rt.find("scene/overlay/tools/open").is_some());
    // The Overlays menu hides it, and Reset brings everything back.
    press(&mut rt, "scene/overlays");
    press(&mut rt, "scene/overlays/tools");
    assert!(dock(&rt, "tools").hidden);
    assert!(rt.find("scene/overlay/tools").is_none());
    rt.update(super::scene_overlays::reset);
    settle(&mut rt);
    let d = dock(&rt, "tools");
    assert!(!d.hidden && !d.collapsed && d.edge == super::scene_overlays::Edge::Top);
}

#[test]
fn a_scene_overlay_floats_as_a_panel_switches_layout_and_docks_back() {
    use super::scene_overlays::{Edge, Layout};
    let mut rt = open();
    settle(&mut rt);
    let dock = |rt: &Runtime<Studio>| super::scene_overlays::dock(rt.state(), "tools");
    let drag_grip = |rt: &mut Runtime<Studio>, to: (f32, f32)| {
        track(rt, &["scene/view", "scene/overlay/tools"]);
        let grip = rt.find("scene/overlay/tools/grip").expect("its grip");
        rt.pointer_move(grip.center().x, grip.center().y);
        rt.frame(FRAME);
        rt.pointer_down(grip.center().x, grip.center().y);
        for step in 1..=8 {
            let t = step as f32 / 8.0;
            rt.pointer_move(
                grip.center().x + (to.0 - grip.center().x) * t,
                grip.center().y + (to.1 - grip.center().y) * t,
            );
            rt.frame(FRAME);
        }
        rt.pointer_up(to.0, to.1);
        settle(rt);
        track(rt, &["scene/view", "scene/overlay/tools"]);
        settle(rt);
    };
    let view = rt.find("scene/view").expect("the Scene view");
    // Let go in the middle of the view: it floats there, as a titled panel.
    drag_grip(&mut rt, (view.center().x, view.center().y));
    let f = dock(&rt).float.expect("floating");
    assert_eq!(f.layout, Layout::Panel);
    let shown = rt.find("scene/overlay/tools").expect("shown");
    assert!(
        shown.left > view.left + 40.0 && shown.top > view.top + 40.0,
        "where it was let go: {shown:?} in {view:?}"
    );
    // Its menu lays it out vertically, in place.
    rt.update(|s| s.local.overlay_menu = Some(("tools", (shown.left + 4.0, shown.top + 4.0))));
    settle(&mut rt);
    press(&mut rt, "scene/overlay/tools/vertical");
    let g = dock(&rt).float.expect("still floating");
    assert_eq!((g.layout, g.at), (Layout::Vertical, f.at));
    let tall = rt.find("scene/overlay/tools").expect("shown");
    assert!(tall.height() > tall.width(), "a column: {tall:?}");
    // The saved form reads back the same.
    let text = dock(&rt).text();
    assert_eq!(
        super::scene_overlays::Dock::parse(&text),
        Some(dock(&rt)),
        "{text}"
    );
    // Dragged to the right edge, it docks there again.
    drag_grip(&mut rt, (view.right - 14.0, view.center().y));
    let d = dock(&rt);
    assert_eq!((d.float, d.edge), (None, Edge::Right), "{d:?}");
}

#[test]
fn a_saved_layout_brings_its_scene_overlays_back() {
    let mut rt = open();
    settle(&mut rt);
    let tools = |rt: &Runtime<Studio>| super::scene_overlays::dock(rt.state(), "tools");
    rt.update(|s| {
        super::scene_overlays::update(s, "tools", |d| {
            d.float = Some(super::scene_overlays::Float {
                at: (160.0, 120.0),
                layout: super::scene_overlays::Layout::Panel,
            })
        });
        s.act("editor.layout.save", json!({"name": "Overlays"}));
        super::scene_overlays::reset(s);
    });
    settle(&mut rt);
    assert!(tools(&rt).float.is_none(), "reset docks it");
    rt.update(|s| s.act("editor.layout.apply", json!({"name": "Overlays"})));
    settle(&mut rt);
    assert_eq!(
        tools(&rt).float.map(|f| f.at),
        Some((160.0, 120.0)),
        "as saved with the layout"
    );
}

#[test]
fn the_draw_mode_menu_keeps_every_entry_and_offers_what_the_renderer_has() {
    use super::unavailable::{effect_available, view_mode_available};
    let mut rt = open();
    settle(&mut rt);
    press(&mut rt, "scene/draw-mode");
    assert!(
        rt.find("scene/draw-mode/overdraw").is_some() && rt.find("scene/effect/Shadows").is_some(),
        "every mode and effect keeps its entry"
    );
    // An entry the renderer does not offer is disabled: pressing it changes nothing.
    assert!(!view_mode_available("wireframe") && !effect_available("Shadows"));
    press(&mut rt, "scene/draw-mode/wireframe");
    assert_ne!(rt.state().editor.state.view_mode, "wireframe");
    press(&mut rt, "scene/effect/Shadows");
    assert!(
        rt.state().editor.state.render_debug.shadows,
        "Shadows stay on"
    );
    // An offered one works.
    assert!(view_mode_available("lit"));
    press(&mut rt, "scene/draw-mode/lit");
    assert!(matches!(
        rt.state().editor.state.view_mode.as_str(),
        "" | "lit"
    ));
}

#[test]
fn dragging_a_box_in_the_scene_selects_what_it_covers() {
    let mut rt = open();
    settle(&mut rt);
    rt.update(|s| s.select(Vec::new(), false));
    let view = rt.find("scene/view").expect("the Scene view");
    // The whole view: every visible, unlocked entity with an origin in front of the camera.
    let (a, b) = (
        (view.left + 2.0, view.top + 2.0),
        (view.right - 2.0, view.bottom - 2.0),
    );
    rt.pointer_move(a.0, a.1);
    rt.frame(FRAME);
    rt.pointer_down(a.0, a.1);
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        rt.pointer_move(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        rt.frame(FRAME);
    }
    assert!(
        rt.state().local.marquee.is_some(),
        "the box shows while dragging"
    );
    rt.pointer_up(b.0, b.1);
    settle(&mut rt);
    let selected = rt.state().editor.state.selection.clone();
    assert!(selected.contains(&"e_box".to_owned()), "{selected:?}");
    assert!(rt.state().local.marquee.is_none());
}

#[test]
fn the_camera_overlay_saves_and_recalls_views() {
    let mut rt = open();
    settle(&mut rt);
    press(&mut rt, "scene/overlay/camera/settings");
    press(&mut rt, "scene/bookmark/save");
    let doc = rt.state().editor.state.doc.clone();
    let saved: Vec<String> = rt.state().editor.state.camera_bookmarks[&doc]
        .keys()
        .cloned()
        .collect();
    assert_eq!(saved.len(), 1, "{saved:?}");
    let before = rt.state().editor.state.camera;
    rt.update(|s| s.editor.state.camera.orbit([1.0, 0.3]));
    assert_ne!(rt.state().editor.state.camera, before);
    press(&mut rt, &format!("scene/bookmark/{}", saved[0]));
    assert_eq!(rt.state().editor.state.camera, before, "recalled");
}

#[test]
fn the_inspector_locks_searches_and_copies_component_values() {
    let mut rt = open();
    settle(&mut rt);
    // Lock on the box, then select the lamp: the Inspector stays on the box.
    press(&mut rt, "inspector/lock");
    rt.update(|s| s.select(vec!["e_lamp".into()], false));
    settle(&mut rt);
    assert!(rt.find("component/MeshRenderer").is_some(), "still the box");
    press(&mut rt, "inspector/lock");
    assert!(
        rt.find("component/MeshRenderer").is_none(),
        "unlocked: the lamp"
    );
    rt.update(|s| s.select(vec!["e_box".into()], false));
    settle(&mut rt);
    // Search narrows to matching properties.
    press(&mut rt, "inspector/search");
    rt.type_text("rough");
    settle(&mut rt);
    assert!(rt.find("field/MeshRenderer.roughness").is_some());
    assert!(
        rt.find("field/Transform.position").is_none(),
        "only what matches"
    );
    rt.update(|s| s.local.inspector_search.clear());
    settle(&mut rt);
    // Copy the box's renderer, change it, paste it back; then reset.
    press(&mut rt, "component/MeshRenderer/more");
    press(&mut rt, "component/MeshRenderer/copy");
    let roughness = |rt: &Runtime<Studio>| {
        rt.state().resolved["e_box"].record["MeshRenderer"]["roughness"].clone()
    };
    rt.update(|s| s.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_box","path":"MeshRenderer.roughness","value":0.9}})], "Rough"));
    settle(&mut rt);
    assert_eq!(roughness(&rt), json!(0.9));
    press(&mut rt, "component/MeshRenderer/more");
    press(&mut rt, "component/MeshRenderer/paste");
    assert_ne!(roughness(&rt), json!(0.9), "pasted back");
    rt.update(|s| s.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_box","path":"MeshRenderer.roughness","value":0.9}})], "Rough"));
    press(&mut rt, "component/MeshRenderer/more");
    press(&mut rt, "component/MeshRenderer/reset");
    assert_ne!(roughness(&rt), json!(0.9), "reset to its default");
    // Move Up puts the renderer above Transform, and the order is saved.
    let top = |rt: &Runtime<Studio>, c: &str| rt.find(&format!("component/{c}")).expect(c).top;
    assert!(top(&rt, "Transform") < top(&rt, "MeshRenderer"));
    press(&mut rt, "component/MeshRenderer/more");
    press(&mut rt, "component/MeshRenderer/up");
    settle(&mut rt);
    assert!(top(&rt, "MeshRenderer") < top(&rt, "Transform"), "moved up");
    let saved = rt
        .state()
        .editor
        .state
        .settings
        .get("inspector.componentOrder")
        .cloned();
    assert!(
        saved
            .as_ref()
            .and_then(Value::as_array)
            .is_some_and(|o| o.starts_with(&[json!("MeshRenderer"), json!("Transform")])),
        "{saved:?}"
    );
    press(&mut rt, "component/MeshRenderer/more");
    press(&mut rt, "component/MeshRenderer/down");
    settle(&mut rt);
    assert!(
        top(&rt, "Transform") < top(&rt, "MeshRenderer"),
        "and back down"
    );
    rt.update(|s| {
        s.act(
            "editor.settings.set",
            json!({"key": "inspector.componentOrder", "reset": true}),
        )
    });
}

#[test]
fn why_profiler_baseline_and_floating_windows_run_from_their_menus() {
    use crate::catalog::Target;
    let mut rt = open();
    settle(&mut rt);
    let run = |rt: &mut Runtime<Studio>, id: &str, target: Target| {
        rt.update(|s| super::commands::run(s, id, &target));
        settle(rt);
    };
    // Why? asks about the selection, then shows the diagnostic service's answer.
    run(&mut rt, "tools.why", Target::None);
    let asked = rt.state().local.prompt.as_ref().map(|p| p.text.clone());
    assert_eq!(asked.as_deref(), Some("e_box not visible"));
    press(&mut rt, "prompt/ok");
    let answer = rt.state().local.why.clone();
    assert!(
        answer.as_ref().is_some_and(|(_, lines)| !lines.is_empty()),
        "{answer:?}"
    );
    press(&mut rt, "why/close");
    assert!(rt.state().local.why.is_none());
    // Set Baseline needs frames; Compare then hides and shows the comparison.
    run(&mut rt, "profiler.compare", Target::None);
    assert!(
        !rt.state().local.profiler_hide_compare,
        "no baseline to compare with"
    );
    rt.update(|s| s.profile = std::sync::Arc::new(sample_profile()));
    run(&mut rt, "profiler.baseline", Target::None);
    assert!(rt.state().editor.profile_baseline.is_some());
    run(&mut rt, "profiler.compare", Target::None);
    assert!(rt.state().local.profiler_hide_compare);
    // Two floating windows: focusing the first brings it to the front.
    rt.update(|s| {
        for panel in ["console", "problems"] {
            s.act(
                "editor.layout.move",
                json!({"panel": panel, "slot": "float", "rect": [40.0, 40.0, 400.0, 300.0]}),
            );
        }
    });
    let order = |rt: &Runtime<Studio>| -> Vec<String> {
        rt.state()
            .editor
            .state
            .floating
            .iter()
            .map(|f| f.id.clone())
            .collect()
    };
    let first = order(&rt).first().cloned().expect("two floating windows");
    run(
        &mut rt,
        "window.focus_window",
        Target::Window(first.clone()),
    );
    assert_eq!(order(&rt).last(), Some(&first), "in front");
    // Commands not built yet say why instead of failing.
    run(&mut rt, "file.revert", Target::None);
    assert!(
        rt.state()
            .local
            .note
            .as_ref()
            .is_some_and(|(n, _)| n.starts_with("Revert"))
    );
}

#[test]
fn history_pages_assets_search_restores_and_render_debug_opens() {
    use crate::catalog::Target;
    let mut rt = open();
    settle(&mut rt);
    let run = |rt: &mut Runtime<Studio>, id: &str| {
        rt.update(|s| super::commands::run(s, id, &Target::None));
        settle(rt);
    };
    // More changes than one page: Older shows the rest, Newer comes back.
    rt.update(|s| {
        for i in 0..super::agents::HISTORY_PAGE {
            let x = i as f64;
            s.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_lamp","path":"Transform.position","value":[x,0,0]}})], "Nudge");
        }
        s.act("editor.layout.show", json!({"panel": "history"}));
    });
    settle(&mut rt);
    assert!(rt.find("history/older").is_some() && rt.find("history/newer").is_none());
    // (The button sits below the page's rows, out of view; the command is the same.)
    run(&mut rt, "history.older");
    assert_eq!(rt.state().local.history_page, 1);
    assert!(rt.find("history/newer").is_some() && rt.find("history/older").is_none());
    press(&mut rt, "history/newer");
    assert_eq!(rt.state().local.history_page, 0);
    // A cleared Assets search comes back.
    rt.update(|s| {
        s.act("editor.workspace", json!({"assetQuery": "crate"}));
        s.local.assets_cleared_query = Some("crate".into());
        s.act("editor.workspace", json!({"assetQuery": ""}));
    });
    run(&mut rt, "assets.search.restore");
    assert_eq!(
        rt.state()
            .editor
            .state
            .searches
            .get("assets")
            .map(String::as_str),
        Some("crate")
    );
    // Rendering Debug opens the Scene's draw-mode menu.
    run(&mut rt, "window.render_debug");
    assert_eq!(rt.state().local.overlay_settings, Some("draw"));
    // Copy Probe State needs the renderer's report first, and says so.
    run(&mut rt, "lighting.probe.copy");
    assert!(rt.state().local.copy.is_none());
}

#[test]
fn drop_to_surface_asks_the_host_and_waits_for_presented_geometry() {
    use crate::catalog::Target;
    let mut rt = open();
    settle(&mut rt);
    rt.update(|s| s.select(Vec::new(), false));
    rt.update(|s| super::commands::run(s, "edit.transform.drop", &Target::None));
    assert!(!rt.state().local.drop_to_surface, "nothing selected");
    rt.update(|s| s.select(vec!["e_box".into()], false));
    rt.update(|s| super::commands::run(s, "edit.transform.drop", &Target::None));
    assert!(
        rt.state().local.drop_to_surface,
        "the host drops it next pass"
    );
    // Before the Scene has presented this scene, the planner refuses rather than guess.
    let scene = crate::studio::viewport::SceneView::default();
    let planned = crate::surface_drop::plan(&rt.state().editor, &scene, |_| None);
    assert!(planned.is_err_and(|e| e.to_string().contains("waiting")));
}

#[test]
fn a_piloted_camera_follows_the_view_and_commits_each_finished_move() {
    use crate::catalog::Target;
    let mut rt = open();
    settle(&mut rt);
    rt.update(|s| {
        s.apply(vec![json!({"cmd":"entity.create","args":{"id":"e_cam","name":"Shot","components":{"Transform":{"position":[0,2,6]},"Camera":{"fov":55.0}}}})], "Camera");
        s.select(vec!["e_cam".into()], false);
        s.act("editor.camera", json!({"projection": "perspective"}));
    });
    settle(&mut rt);
    rt.update(|s| super::commands::run(s, "view.camera.pilot", &Target::None));
    assert!(
        rt.state().local.pilot.active(),
        "piloting the selected Camera"
    );
    let position =
        |rt: &Runtime<Studio>| rt.state().resolved["e_cam"].record["Transform"]["position"].clone();
    let before = position(&rt);
    let revision = rt.state().editor.host.store().revision();
    // A move under way stays transient; when it ends it lands as one edit.
    rt.update(|s| {
        s.local.navigating = true;
        s.editor.state.camera.pan([40.0, 0.0]);
        super::scene::poll_pilot(s, true);
    });
    assert_eq!(
        rt.state().editor.host.store().revision(),
        revision,
        "still moving"
    );
    rt.update(|s| {
        s.local.navigating = false;
        super::scene::poll_pilot(s, false);
        s.resolve();
    });
    assert_eq!(
        rt.state().editor.host.store().revision(),
        revision + 1,
        "one undoable edit"
    );
    assert_ne!(position(&rt), before, "the Camera moved with the view");
    rt.update(|s| super::commands::run(s, "view.camera.pilot", &Target::None));
    assert!(!rt.state().local.pilot.active(), "stopped");
}

#[test]
fn keyboard_shortcuts_record_clear_and_reset_a_binding() {
    use stillkit::el::{KeyCode, Modifiers};
    let mut rt = open();
    settle(&mut rt);
    rt.update(|s| super::commands::run(s, "edit.shortcuts", &crate::catalog::Target::None));
    settle(&mut rt);
    assert!(
        rt.find("shortcuts/edit.duplicate").is_some(),
        "every command is listed"
    );
    let own = |rt: &Runtime<Studio>| {
        rt.state()
            .editor
            .state
            .settings
            .get("input.shortcuts")
            .cloned()
    };
    let keys = |rt: &Runtime<Studio>| super::commands::keys_of(rt.state(), "edit.duplicate");
    // Record: the next chord is the person's binding, in force at once.
    rt.update(|s| {
        if let Some(v) = s.local.settings.as_mut() {
            v.query = "duplicate".into();
        }
    });
    settle(&mut rt);
    press(&mut rt, "shortcuts/edit.duplicate/record");
    assert_eq!(rt.state().local.recording_shortcut, Some("edit.duplicate"));
    rt.key_with(KeyCode::of('J').0, Modifiers::CTRL);
    settle(&mut rt);
    assert_eq!(own(&rt), Some(json!({"edit.duplicate": "Ctrl+J"})));
    assert_eq!(keys(&rt), super::commands::shortcut("Ctrl+J"));
    assert!(rt.state().local.recording_shortcut.is_none());
    // Clear unbinds it; Reset gives it back to the preset.
    // Away first: Record's tooltip, just shown, would take the press beside it.
    rt.pointer_move(5.0, 5.0);
    settle(&mut rt);
    press(&mut rt, "shortcuts/edit.duplicate/clear");
    assert_eq!(keys(&rt), None);
    press(&mut rt, "shortcuts/edit.duplicate/reset");
    assert_eq!(
        own(&rt).and_then(|o| o.get("edit.duplicate").cloned()),
        None
    );
    assert_eq!(keys(&rt), super::commands::shortcut("Ctrl+D"));
}

#[test]
fn the_lighting_list_mixes_lights_in_place() {
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "window.lighting", &crate::catalog::Target::None));
    settle(&mut rt);
    let light = |rt: &Runtime<Studio>| rt.state().resolved["e_sun"].record["Light"].clone();
    // Intensity: typed in the row, Enter applies.
    press(&mut rt, "lighting/e_sun/intensity");
    rt.key_with(
        stillkit::el::KeyCode::of('A').0,
        stillkit::el::Modifiers::CTRL,
    );
    rt.type_text("12000");
    rt.key(stillkit::el::KeyCode::ENTER.0);
    settle(&mut rt);
    assert_eq!(light(&rt)["intensity"], json!(12000.0));
    // A colour temperature from the swatch's menu.
    press(&mut rt, "lighting/e_sun/color");
    press(&mut rt, "lighting/e_sun/kelvin/1900");
    let color = light(&rt)["color"].clone();
    assert!(
        color[0].as_f64().unwrap() > color[2].as_f64().unwrap(),
        "candlelight is warm: {color}"
    );
    // The eye hides it in the Scene only.
    press(&mut rt, "lighting/e_sun/eye");
    assert!(rt.state().editor.state.hidden.contains("e_sun"));
    // Rows that are not lights have no mixer.
    assert!(rt.find("lighting/e_post/intensity").is_none());
    // The curve runs warm to cool, its brightest channel 1.
    let [warm, cool] = [
        super::lighting::kelvin(1900.0),
        super::lighting::kelvin(10000.0),
    ];
    assert!(
        warm[0] == 1.0 && warm[2] < 0.2 && cool[2] == 1.0 && cool[0] < 1.0,
        "{warm:?} {cool:?}"
    );
}

#[test]
fn brush_sliders_set_by_press_and_drag() {
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "window.mesh-paint", &crate::catalog::Target::None));
    settle(&mut rt);
    let press_at = |rt: &mut Runtime<Studio>, key: &str, along: f32| {
        let r = rt.find(key).unwrap_or_else(|| panic!("{key} shows"));
        let (x, y) = (r.left + r.width() * along, r.center().y);
        rt.pointer_move(x, y);
        rt.frame(FRAME);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    };
    let paint = |rt: &Runtime<Studio>| rt.state().editor.state.paint.unwrap_or_default();
    press_at(&mut rt, "paint/strength/track", 0.75);
    assert!(
        (paint(&rt).strength - 0.75).abs() < 0.05,
        "{}",
        paint(&rt).strength
    );
    // Radius runs by magnitude: halfway is the range's geometric middle, not 50 m.
    press_at(&mut rt, "paint/radius/track", 0.5);
    let radius = paint(&rt).radius;
    assert!(radius > 0.3 && radius < 1.5, "{radius}");
}

#[test]
fn an_assets_details_say_what_it_is_and_holds() {
    let mut rt = open();
    scan(&mut rt);
    rt.update(|s| {
        let path = s
            .assets
            .as_ref()
            .and_then(|b| b.items.iter().find(|a| a.path.ends_with("main.scene")))
            .map(|a| a.path.clone())
            .unwrap();
        s.act("editor.workspace", json!({"asset": path}));
        s.local.inspect_asset = true;
    });
    settle(&mut rt);
    let labels: Vec<String> = rt
        .tree()
        .iter()
        .filter_map(|shown| shown.label.as_deref().map(str::to_owned))
        .collect();
    let has = |want: &str| labels.iter().any(|l| l.starts_with(want));
    assert!(has("Kind: Scene"), "{labels:?}");
    assert!(has("Size: ") && has("Modified: "));
    assert!(has("Entities: 4"));
    assert!(has("Components: Transform 4"), "most common first");
}

/// A few messages as a session might have them, for the Console and Problems captures.
fn sample_logs(s: &mut Studio) {
    use ara_diag::Severity;
    let row = |seq: u64,
               severity: Severity,
               code: &str,
               message: &str,
               origin: &str,
               source: Option<&str>,
               count: u64| {
        let number = ara_diag::Code::parse(code).expect("sample diagnostic code is valid");
        let diagnostic = match severity {
            Severity::Error => ara_diag::Diagnostic::error(number, message),
            Severity::Warning => ara_diag::Diagnostic::warning(number, message),
            _ => ara_diag::Diagnostic::info(number, message),
        };
        super::state::LogRow {
            seq,
            severity,
            code: code.into(),
            message: message.into(),
            origin: origin.into(),
            at: seq * 1_000,
            source: source.map(str::to_owned),
            count,
            diagnostic: std::sync::Arc::new(diagnostic),
        }
    };
    s.logs = vec![
        row(
            1,
            Severity::Info,
            "ARA4508",
            "Project opened: 4 entities in levels/main.scene",
            "engine",
            None,
            1,
        ),
        row(
            2,
            Severity::Info,
            "ARA0911",
            "C# modules compiled in 1.8 s",
            "scripts",
            None,
            1,
        ),
        row(
            3,
            Severity::Warning,
            "ARA2104",
            "Texture rocks_albedo.png is 4096×4096 without mipmaps; it streams slowly",
            "import",
            Some("content/art/rocks_albedo.png"),
            1,
        ),
        row(
            4,
            Severity::Warning,
            "ARA3302",
            "Light Sun casts shadows past its cascade range",
            "render",
            None,
            12,
        ),
        row(
            5,
            Severity::Error,
            "ARA0912",
            "PlayerControl.Update threw NullReferenceException at PlayerControl.cs:42",
            "scripts",
            Some("content/scripts/PlayerControl.cs"),
            1,
        ),
    ];
    s.local.log_seen = 5;
    // The error open in the details pane, as after a click.
    s.local.console_open = Some(5);
}

/// Agent clients as a probe might find them (no client's CLI runs in tests).
fn found_clients(s: &mut Studio) {
    use ara_agents::{Client, Setup};
    let setups = &mut s.local.agent_setups;
    setups.probed = true;
    setups.ara = Some(PathBuf::from("/opt/ara/ara"));
    setups.setup.insert(Client::Claude, Setup::Connected);
    setups.setup.insert(Client::Codex, Setup::Available);
    setups.setup.insert(Client::Cursor, Setup::Missing);
    setups.setup.insert(
        Client::OpenCode,
        Setup::Failed("opencode.json is not JSON".into()),
    );
    super::agents::draw_logos(s);
}

#[test]
fn agent_clients_connect_or_show_their_manual_steps() {
    let mut rt = open();
    rt.update(|s| {
        found_clients(s);
        s.act("editor.layout.show", json!({"panel": "agents"}));
    });
    settle(&mut rt);
    // Each client offers the one action that fits what was found.
    assert!(
        rt.find("agents/codex/connect").is_some(),
        "installed: Connect"
    );
    assert!(rt.find("agents/opencode/retry").is_some(), "failed: Retry");
    assert!(
        rt.find("agents/claude/connect").is_none(),
        "already connected"
    );
    // Where the editor cannot connect it: manual steps, copied as a whole.
    press(&mut rt, "agents/cursor/manual");
    press(&mut rt, "agents/cursor/copy");
    let copied = rt.state().local.copy.clone().unwrap_or_default();
    assert!(
        copied.contains("/opt/ara/ara") && copied.contains("mcp"),
        "{copied}"
    );
    // Any other client: the raw command, with its own Copy (below the fold here).
    assert!(rt.find("agents/connect/copy").is_some());
}

#[test]
fn a_hovered_hierarchy_row_offers_its_eye_and_lock() {
    let mut rt = open();
    settle(&mut rt);
    let row = rt.find("row/e_lamp").unwrap();
    rt.pointer_move(row.center().x, row.center().y);
    settle(&mut rt);
    assert_eq!(rt.state().local.hierarchy_hover.as_deref(), Some("e_lamp"));
    // Elsewhere: no row stays hovered.
    let inspector = rt.find("component/Transform").unwrap();
    rt.pointer_move(inspector.center().x, inspector.center().y);
    settle(&mut rt);
    super::hierarchy::unhover(&mut rt);
    assert!(rt.state().local.hierarchy_hover.is_none());
}

#[test]
fn the_startup_scene_is_set_from_build_or_a_scene_asset() {
    use crate::catalog::Target;
    let mut rt = open();
    settle(&mut rt);
    let main = |rt: &Runtime<Studio>| super::build_page::main_scene(rt.state());
    let first = main(&rt).expect("the project starts with a scene");
    let original = rt.state().editor.state.doc.clone();
    let storage = {
        let workspace = rt.state().editor.host.store().workspace();
        workspace.path_of(&original).unwrap().to_owned()
    };
    // A new scene, open now, becomes the one the build starts with.
    rt.update(|s| super::commands::run(s, "file.new_scene", &Target::None));
    settle(&mut rt);
    assert_ne!(rt.state().editor.state.doc, original);
    rt.update(|s| super::commands::run(s, "build.main_scene", &Target::None));
    let second = main(&rt).unwrap();
    assert_ne!(second, first);
    // A scene asset's menu puts the first back, as one undoable change.
    rt.update(|s| super::commands::run(s, "assets.set_main", &Target::Asset(storage.clone())));
    assert_eq!(main(&rt).as_deref(), Some(first.as_str()));
    rt.update(|s| super::commands::run(s, "edit.undo", &Target::None));
    assert_eq!(main(&rt), Some(second));
}

#[test]
fn component_headers_say_what_the_component_is() {
    let mut rt = open();
    settle(&mut rt);
    for component in ["Transform", "MeshRenderer"] {
        let key = format!("component/{component}/about");
        assert!(rt.find(&key).is_some(), "{key}");
    }
    let doc = rt
        .state()
        .editor
        .host
        .store()
        .schema()
        .get("MeshRenderer")
        .map(|t| t.doc.clone());
    assert!(
        doc.is_some_and(|d| !d.trim().is_empty()),
        "the schema documents it"
    );
}

#[test]
fn a_new_material_opens_docked_beside_the_scene() {
    let mut rt = open();
    settle(&mut rt);
    rt.update(|s| super::asset_create::create(s, "material"));
    settle(&mut rt);
    let state = &rt.state().editor.state;
    let floating = state
        .floating
        .iter()
        .any(|f| super::dock::window_title(&f.root).contains("Material"));
    assert!(!floating, "a new material's page docks, not floats");
    assert!(crate::state::panel_visible(state, "material"));
}

#[test]
fn the_status_bar_shows_the_newest_problem_and_opens_it() {
    let mut rt = open();
    rt.update(|s| {
        sample_logs(s);
        s.local.console_open = None;
    });
    settle(&mut rt);
    assert!(
        rt.find("status/log").is_some(),
        "the newest warning or error shows"
    );
    press(&mut rt, "status/log");
    assert_eq!(
        rt.state().local.console_open,
        Some(5),
        "the error, in the Console"
    );
    assert!(crate::state::panel_visible(
        &rt.state().editor.state,
        "console"
    ));
}

#[test]
fn a_settings_slider_previews_while_dragged_and_commits_once() {
    let mut rt = open();
    rt.update(|s| {
        super::commands::run(s, "edit.preferences", &crate::catalog::Target::None);
        if let Some(v) = s.local.settings.as_mut() {
            v.query = "camera speed".into();
        }
    });
    settle(&mut rt);
    let key = "settings/viewport.cameraSpeed/track";
    let r = rt.find(key).unwrap_or_else(|| panic!("{key} shows"));
    let speed = |rt: &Runtime<Studio>| {
        rt.state()
            .editor
            .state
            .settings
            .get("viewport.cameraSpeed")
            .and_then(Value::as_f64)
    };
    let before = speed(&rt);
    let y = r.center().y;
    rt.pointer_move(r.left + 2.0, y);
    rt.frame(FRAME);
    rt.pointer_down(r.left + 2.0, y);
    rt.pointer_move(r.left + r.width() * 0.5, y);
    rt.frame(FRAME);
    assert!(rt.state().local.setting_drag.is_some(), "previewing");
    assert_eq!(speed(&rt), before, "nothing committed mid-drag");
    rt.pointer_up(r.left + r.width() * 0.5, y);
    settle(&mut rt);
    assert!(rt.state().local.setting_drag.is_none());
    let after = speed(&rt).unwrap();
    assert!(after > 15.0 && after < 35.0, "about halfway: {after}");
}

#[test]
fn the_scene_header_has_the_scenes_own_menu() {
    let mut rt = open();
    settle(&mut rt);
    let header = rt.find("hierarchy/scene").unwrap();
    rt.right_click(header.center().x, header.center().y);
    settle(&mut rt);
    let context = rt.state().local.context.clone();
    assert!(
        matches!(&context, Some((crate::catalog::menus::Context::Scene, crate::catalog::Target::Asset(p), _)) if p.ends_with("main.scene")),
        "{context:?}"
    );
    for id in [
        "file.save",
        "assets.set_main",
        "assets.show",
        "create.empty",
    ] {
        assert!(rt.find(&format!("cmd/{id}")).is_some(), "{id} is offered");
    }
    // Create Empty from the scene's menu goes at the root, not under the selection.
    let before = rt.state().resolved.len();
    press(&mut rt, "cmd/create.empty");
    let made: Vec<_> = rt.state().editor.state.selection.clone();
    assert_eq!(rt.state().resolved.len(), before + 1);
    assert!(
        rt.state().resolved[&made[0]].parent.is_none(),
        "a root entity"
    );
}

#[test]
fn the_assets_footer_names_the_selection_or_the_folder() {
    let mut rt = open();
    scan(&mut rt);
    rt.update(|s| {
        s.act("editor.layout.show", json!({"panel": "assets"}));
        s.act(
            "editor.workspace",
            json!({"folder": ara_doc::AUTHORING_ROOT, "asset": ""}),
        );
    });
    settle(&mut rt);
    let footer = |rt: &Runtime<Studio>| {
        rt.tree()
            .into_iter()
            .find(|shown| shown.key.as_deref() == Some("assets/footer"))
            .and_then(|shown| shown.label.as_deref().map(str::to_owned))
    };
    let folder = footer(&rt).unwrap_or_default();
    assert!(
        folder.contains(" items") || folder.contains(" item"),
        "{folder}"
    );
    // The footer says the count (and the folder below the root), not "Project" again.
    assert!(!folder.contains("Project"), "{folder}");
    let path = rt
        .state()
        .assets
        .as_ref()
        .and_then(|b| b.items.iter().find(|a| a.path.ends_with("main.scene")))
        .map(|a| a.path.clone())
        .unwrap();
    rt.update(|s| s.act("editor.workspace", json!({"asset": path.clone()})));
    settle(&mut rt);
    assert_eq!(footer(&rt), Some(path), "the selected asset's path");
}

#[test]
fn the_inspectors_debug_mode_shows_the_record() {
    let mut rt = open();
    settle(&mut rt);
    press(&mut rt, "inspector/raw");
    assert!(rt.find("inspector/record").is_some(), "the record shows");
    assert!(
        rt.find("component/Transform").is_none(),
        "instead of the fields"
    );
    press(&mut rt, "inspector/record/copy");
    let copied = rt.state().local.copy.clone().unwrap_or_default();
    assert!(
        copied.contains("MeshRenderer") && copied.contains("Transform"),
        "{copied}"
    );
    press(&mut rt, "inspector/raw");
    assert!(
        rt.find("component/Transform").is_some(),
        "back to the fields"
    );
}

#[test]
fn the_profiler_shows_scopes_under_their_callers() {
    let mut rt = open();
    rt.update(|s| {
        s.profile = std::sync::Arc::new(sample_profile());
        super::commands::run(s, "window.profiler", &crate::catalog::Target::None);
    });
    settle(&mut rt);
    press(&mut rt, "profiler/tree");
    let parent = "profiler/call/studio.scene";
    let child = "profiler/call/studio.scene/scene.render";
    assert!(rt.find(parent).is_some(), "a root scope");
    assert!(rt.find(child).is_none(), "its callee waits until it opens");
    press(&mut rt, parent);
    assert!(rt.find(child).is_some(), "opened: what it called");
    press(&mut rt, "profiler/flat");
    assert!(rt.find(parent).is_none(), "the flat list again");
}

#[test]
fn problems_list_files_with_errors_first() {
    let mut rt = open();
    rt.update(|s| {
        sample_logs(s);
        s.act("editor.layout.show", json!({"panel": "problems"}));
    });
    settle(&mut rt);
    let top = |rt: &Runtime<Studio>, seq: u64| rt.find(&format!("problems/{seq}")).map(|r| r.top);
    // The error (5) above both warnings (3 and 4), though its file sorts last by name.
    let (error, warning) = (top(&rt, 5).unwrap(), top(&rt, 4).unwrap());
    assert!(error < warning, "{error} < {warning}");
}

#[test]
fn a_console_rows_menu_copies_it() {
    let mut rt = open();
    rt.update(|s| {
        sample_logs(s);
        s.local.console_open = None;
        s.act("editor.layout.show", json!({"panel": "console"}));
    });
    settle(&mut rt);
    let row = rt.find("console/row/4").unwrap();
    rt.right_click(row.center().x, row.center().y);
    settle(&mut rt);
    assert_eq!(rt.state().local.console_open, Some(4), "its details open");
    press(&mut rt, "cmd/console.copy");
    let copied = rt.state().local.copy.clone().unwrap_or_default();
    assert!(copied.starts_with("ARA3302: Light Sun"), "{copied}");
}

#[test]
fn empty_hierarchy_space_the_profiler_and_the_status_bar_have_menus() {
    use crate::catalog::menus::Context;
    let mut rt = open();
    settle(&mut rt);
    let kind = |rt: &Runtime<Studio>| rt.state().local.context.as_ref().map(|c| c.0);
    let right_click = |rt: &mut Runtime<Studio>, x: f32, y: f32| {
        rt.update(|s| s.local.context = None);
        settle(rt);
        rt.right_click(x, y);
        settle(rt);
    };
    // Below the last row: the Hierarchy's own menu; on a row, the entity's.
    let post = rt.find("row/e_post").unwrap();
    right_click(&mut rt, post.center().x, post.bottom + 60.0);
    assert_eq!(kind(&rt), Some(Context::Hierarchy));
    right_click(&mut rt, post.center().x, post.center().y);
    assert_eq!(kind(&rt), Some(Context::Entity));
    let status = rt.find("status").unwrap();
    right_click(&mut rt, status.left + 300.0, status.center().y);
    assert_eq!(kind(&rt), Some(Context::Status));
    rt.update(|s| {
        s.profile = std::sync::Arc::new(sample_profile());
        super::commands::run(s, "window.profiler", &crate::catalog::Target::None);
    });
    settle(&mut rt);
    let strip = rt
        .find("profiler/track")
        .or_else(|| rt.find("profiler/calls"))
        .unwrap();
    right_click(&mut rt, strip.center().x, strip.center().y);
    assert_eq!(kind(&rt), Some(Context::Profiler));
}

#[test]
fn the_palette_finds_settings() {
    let mut rt = open();
    rt.update(|s| s.local.palette = Some(("camera speed".into(), 0)));
    settle(&mut rt);
    press(&mut rt, "palette/0");
    let view = rt.state().local.settings.clone();
    assert!(
        view.as_ref()
            .is_some_and(|v| v.query == "Camera speed" && v.scope == crate::settings::Scope::User),
        "{view:?}"
    );
}

#[test]
fn a_splitter_drag_is_kept_in_the_layout_without_a_failure() {
    let mut rt = open();
    settle(&mut rt);
    let root = rt.state().editor.host.root().to_owned();
    let ratio = |rt: &Runtime<Studio>| match &rt.state().editor.state.layout {
        ara_ui::docking::DockNode::Split { ratio, .. } => *ratio,
        _ => panic!("the default layout splits at its root"),
    };
    let before = ratio(&rt);
    // The host reports the splits' rects; the drag reads them.
    let keys = super::dock::rect_keys(&rt.state().editor.state.layout);
    track(
        &mut rt,
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let hit = rt.find("splitter/split/").expect("the root splitter");
    let (x, y) = (hit.center().x, hit.center().y);
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    rt.pointer_down(x, y);
    rt.pointer_move(x + 60.0, y + 60.0);
    rt.frame(FRAME);
    rt.pointer_up(x + 60.0, y + 60.0);
    settle(&mut rt);
    // The authority keeps the new ratio (a later sync leaves it), and writes the file.
    let after = ratio(&rt);
    assert!((after - before).abs() > 0.01, "{before} -> {after}");
    rt.update(|s| {
        s.act("editor.focus", json!({"panel": "hierarchy"}));
    });
    assert!(
        (ratio(&rt) - after).abs() < 1e-4,
        "kept across the next action"
    );
    let file = root.join(".ara/editor-layout.json");
    let written = (0..200).any(|_| {
        std::thread::sleep(Duration::from_millis(10));
        std::fs::read_to_string(&file).is_ok_and(|text| {
            text.contains(&format!("{after}")) || text.contains(&format!("{:.3}", after))
        })
    });
    assert!(written, "{} holds the new ratio {after}", file.display());
    // Nothing failed on the way: no error reached the status bar or the Console.
    rt.update(|s| {
        s.poll_logs();
    });
    settle(&mut rt);
    let errors: Vec<_> = rt
        .state()
        .logs
        .iter()
        .filter(|r| r.severity == ara_diag::Severity::Error)
        .map(|r| r.message.clone())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(rt.find("status/log").is_none());
}

#[test]
fn menus_cascade_their_submenus() {
    let mut rt = open();
    settle(&mut rt);
    press(&mut rt, "menu/File");
    assert!(
        rt.find("cmd/build.current").is_none(),
        "folded under Build & Export"
    );
    let hover = |rt: &mut Runtime<Studio>, key: &str| {
        let r = rt.find(key).unwrap_or_else(|| panic!("{key} shows"));
        rt.pointer_move(r.center().x, r.center().y);
        settle(rt);
    };
    hover(&mut rt, "sub/Build & Export");
    assert!(
        rt.find("cmd/build.current").is_some(),
        "beside it, on hover"
    );
    hover(&mut rt, "cmd/file.save");
    assert!(
        rt.find("cmd/build.current").is_none(),
        "another row closes it"
    );
    assert_eq!(rt.state().local.menu, Some("File"), "the menu stays");
}

#[test]
fn keymap_presets_and_rebinds_drive_the_studios_keys_and_labels() {
    use stillkit::el::{KeyCode, Modifiers};
    let mut rt = open();
    settle(&mut rt);
    let all = rt.state().resolved.len();
    let selected = |rt: &Runtime<Studio>| rt.state().editor.state.selection.len();
    // Set in memory, as a settings edit leaves them, so the test profile stays default.
    let set = |rt: &mut Runtime<Studio>, key: &str, value: Value| {
        rt.update(|s| {
            s.editor.state.settings.insert(key.into(), value);
            s.editor.state.selection.clear();
        });
    };
    rt.key_with(KeyCode::of('A').0, Modifiers::CTRL);
    assert_eq!(selected(&rt), all, "Ara: Ctrl+A selects all");
    // Blender's preset: A selects all, Ctrl+A does not.
    set(&mut rt, "input.keymap", json!("blender"));
    rt.key_with(KeyCode::of('A').0, Modifiers::CTRL);
    assert_eq!(selected(&rt), 0);
    rt.key(KeyCode::of('A').0);
    assert_eq!(selected(&rt), all, "Blender: A selects all");
    // The person's own binding wins over the preset, and the menus say so.
    set(
        &mut rt,
        "input.shortcuts",
        json!({"edit.select_all": "Ctrl+Shift+L"}),
    );
    rt.key_with(KeyCode::of('L').0, Modifiers::CTRL | Modifiers::SHIFT);
    assert_eq!(selected(&rt), all, "rebound");
    assert_eq!(
        super::commands::keys_of(rt.state(), "edit.select_all"),
        super::commands::shortcut("Ctrl+Shift+L")
    );
}

#[test]
fn the_hierarchy_filters_by_component_type_and_name() {
    let mut rt = open();
    rt.update(|s| s.local.hierarchy_query = "t:light".into());
    settle(&mut rt);
    assert!(rt.find("row/e_sun").is_some(), "the Sun has a Light");
    assert!(rt.find("row/e_box").is_none());
    rt.update(|s| s.local.hierarchy_query = "t:light lamp".into());
    settle(&mut rt);
    assert!(rt.find("row/e_sun").is_none(), "every term must match");
    rt.update(|s| s.local.hierarchy_query = "bo".into());
    settle(&mut rt);
    assert!(rt.find("row/e_box").is_some() && rt.find("row/e_sun").is_none());
}

#[test]
fn isolate_hides_the_rest_and_runs_again_to_show_it() {
    let mut rt = open();
    settle(&mut rt);
    rt.update(|s| s.select(vec!["e_lamp".into()], false));
    rt.update(|s| super::commands::run(s, "view.isolate", &crate::catalog::Target::None));
    settle(&mut rt);
    let hidden = rt.state().editor.state.hidden.clone();
    assert!(
        hidden.contains("e_box") && !hidden.contains("e_lamp"),
        "{hidden:?}"
    );
    rt.update(|s| super::commands::run(s, "view.isolate", &crate::catalog::Target::None));
    settle(&mut rt);
    assert!(rt.state().editor.state.hidden.is_empty(), "shown again");
    // Copy Reference gives the entity's @scene#id.
    rt.update(|s| super::commands::run(s, "edit.copy_reference", &crate::catalog::Target::None));
    assert!(
        rt.state()
            .local
            .copy
            .as_deref()
            .is_some_and(|r| r.ends_with("#e_lamp"))
    );
}

#[test]
fn file_play_and_assets_commands_run_in_the_studio() {
    let mut rt = open();
    settle(&mut rt);
    let run = |rt: &mut Runtime<Studio>, id: &str| {
        rt.update(|s| super::commands::run(s, id, &crate::catalog::Target::None));
        settle(rt);
    };
    // Open Scene lists the project's scenes in the palette.
    run(&mut rt, "file.open");
    assert_eq!(
        rt.state().local.palette.as_ref().map(|p| p.0.as_str()),
        Some("#")
    );
    rt.update(|s| s.local.palette = None);
    // Play options are settings.
    run(&mut rt, "play.mode.host");
    run(&mut rt, "play.clients.more");
    let settings = &rt.state().editor.state.settings;
    assert_eq!(settings["play.mode"], json!("host"));
    assert_eq!(settings["play.clients"], json!(2.0));
    // Favourite the open folder, then scope searches to it.
    run(&mut rt, "assets.favourite.toggle");
    assert!(!rt.state().editor.state.asset_favourites.is_empty());
    run(&mut rt, "assets.search.scope");
    assert!(rt.state().editor.state.asset_search_folder_only);
    // Exit asks the host to close (with the documents written, nothing is left to ask about).
    rt.update(|s| s.editor.host.flush());
    run(&mut rt, "file.exit");
    assert!(rt.state().local.quit);
}

/// A human session's `daemon.shutdown` closes the studio like File › Exit, so tools that stop the
/// editor through the protocol (the installed-package smoke, Hub) are not left waiting for it.
#[test]
fn a_human_sessions_daemon_shutdown_closes_the_studio() {
    let mut rt = open();
    assert!(!super::closing(rt.state()));
    rt.update(|s| {
        let begun = s
            .editor
            .call_for(
                ara_proto::ConnId::LOCAL,
                "session.begin",
                json!({"client": "test", "author": "human", "label": "stop"}),
            )
            .expect("a human session");
        s.editor
            .call_for(
                ara_proto::ConnId::LOCAL,
                "daemon.shutdown",
                json!({"session": begun["session"]}),
            )
            .expect("a human session may stop the daemon");
    });
    assert!(super::closing(rt.state()));
}

#[test]
fn the_build_window_offers_a_profile_and_starts_creating_one() {
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "build.profiles", &crate::catalog::Target::None));
    settle(&mut rt);
    assert!(rt.find("build/build").is_some(), "the Build window docks");
    assert!(
        rt.find("build/create").is_some(),
        "a project without profiles offers one"
    );
    assert!(rt.find("build/package").is_some() && rt.find("build/format").is_some());
    press(&mut rt, "build/create");
    let started = rt.state().builder.job.is_some()
        || !crate::build::presets(rt.state().editor.host.root()).is_empty();
    assert!(started, "creating the default profile started");
}

#[test]
fn view_selection_and_transform_commands_run_in_the_studio() {
    let mut rt = open();
    settle(&mut rt);
    let run = |rt: &mut Runtime<Studio>, id: &str| {
        rt.update(|s| super::commands::run(s, id, &crate::catalog::Target::None));
        settle(rt);
    };
    // The renderer's view modes and effects refuse until it offers them (`unavailable`).
    run(&mut rt, "view.mode.lit_wireframe");
    assert_ne!(rt.state().editor.state.view_mode, "litWireframe");
    run(&mut rt, "view.debug.shadows");
    assert!(rt.state().editor.state.render_debug.shadows);
    run(&mut rt, "view.mode.lit");
    run(&mut rt, "view.show.grid");
    assert_eq!(
        rt.state().editor.state.settings["viewport.grid"],
        json!(false)
    );
    run(&mut rt, "view.camera.top");
    assert!(rt.state().editor.state.camera.pitch > 1.5, "looking down");
    run(&mut rt, "view.hide_overlays");
    assert!(
        rt.find("scene/overlay/tools").is_none(),
        "Hide Overlays clears the image"
    );
    run(&mut rt, "view.hide_overlays");
    assert!(rt.find("scene/overlay/tools").is_some());
    // The box moved, then reset.
    rt.update(|s| s.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_box","path":"Transform.position","value":[4,0,0]}})], "Move"));
    assert_eq!(position(&rt, "e_box"), json!([4, 0, 0]));
    run(&mut rt, "transform.reset_position");
    assert_ne!(position(&rt, "e_box"), json!([4, 0, 0]));
    run(&mut rt, "edit.invert_selection");
    assert!(
        !rt.state()
            .editor
            .state
            .selection
            .contains(&"e_box".to_owned())
    );
}

#[test]
fn copy_paste_fields_components_and_filters_run_in_the_studio() {
    let mut rt = open();
    settle(&mut rt);
    let run = |rt: &mut Runtime<Studio>, id: &str, target: crate::catalog::Target| {
        rt.update(|s| super::commands::run(s, id, &target));
        settle(rt);
    };
    use crate::catalog::Target;
    // Copy the box and paste it: a second box, selected.
    let before = rt.state().resolved.len();
    run(&mut rt, "edit.copy", Target::None);
    run(&mut rt, "edit.paste", Target::None);
    assert_eq!(rt.state().resolved.len(), before + 1);
    let pasted = rt.state().editor.state.selection.clone();
    assert!(pasted.len() == 1 && pasted[0] != "e_box", "{pasted:?}");
    // A field's value copies to another entity.
    rt.update(|s| s.select(vec!["e_box".into()], false));
    run(
        &mut rt,
        "field.copy",
        Target::Field("Transform.position".into()),
    );
    assert!(rt.state().local.field_clipboard.is_some());
    // Removing a component from the component menu.
    run(
        &mut rt,
        "component.remove",
        Target::Component("MeshRenderer".into()),
    );
    assert!(
        rt.state().resolved["e_box"]
            .record
            .get("MeshRenderer")
            .is_none()
    );
    // Hierarchy filters type the search for you.
    run(&mut rt, "hierarchy.filter.lights", Target::None);
    assert_eq!(rt.state().local.hierarchy_query, "t:light");
    assert!(rt.find("row/e_sun").is_some() && rt.find("row/e_lamp").is_none());
}

#[test]
fn the_console_searches_collapses_and_clear_stays_clear() {
    let mut rt = open();
    let row = |seq: u64, message: &str| super::state::LogRow {
        seq,
        severity: ara_diag::Severity::Warning,
        code: crate::codes::DIAGNOSTIC_SAMPLE.to_string(),
        message: message.into(),
        origin: "engine".into(),
        at: 0,
        source: None,
        count: 1,
        diagnostic: std::sync::Arc::new(ara_diag::Diagnostic::warning(
            crate::codes::DIAGNOSTIC_SAMPLE,
            message,
        )),
    };
    rt.update(|s| {
        s.act("editor.layout.show", json!({"panel": "console"}));
        s.logs = vec![row(1, "same"), row(2, "same"), row(3, "other")];
        s.local.log_seen = 3;
    });
    settle(&mut rt);
    assert!(rt.find("console/row/2").is_some() && rt.find("console/row/3").is_some());
    press(&mut rt, "console/collapse");
    assert!(
        rt.find("console/row/1").is_none(),
        "repeats merge into the latest"
    );
    assert!(rt.find("console/row/2").is_some());
    rt.update(|s| s.local.console_search = "oth".into());
    settle(&mut rt);
    assert!(rt.find("console/row/2").is_none() && rt.find("console/row/3").is_some());
    press(&mut rt, "console/clear");
    rt.update(|s| {
        s.poll_logs();
    });
    assert!(
        rt.state().logs.iter().all(|r| r.seq > 3),
        "cleared records stay gone"
    );
    press(&mut rt, "console/error-pause");
    assert_eq!(
        rt.state().editor.state.settings["console.errorPause"],
        json!(true)
    );
}

#[test]
fn the_game_view_letterboxes_to_its_aspect_and_maximizes() {
    let mut rt = open();
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "game"})));
    settle(&mut rt);
    track(&mut rt, &["game/area"]);
    press(&mut rt, "game/aspect");
    press(&mut rt, "game/aspect/4x3");
    rt.update(|_| {});
    settle(&mut rt);
    let view = rt.find("game/view").expect("the Game view");
    let ratio = view.width() / view.height();
    assert!(
        (ratio - 4.0 / 3.0).abs() < 0.02,
        "4:3 letterbox, got {ratio}"
    );
    // Window › Maximize fills the dock with the Game view alone.
    rt.update(|s| {
        super::commands::run(
            s,
            "window.maximize",
            &crate::catalog::Target::Panel("game".into()),
        )
    });
    settle(&mut rt);
    assert!(
        rt.find("tab/hierarchy").is_none(),
        "only the Game view shows"
    );
    rt.update(|s| {
        super::commands::run(
            s,
            "window.maximize",
            &crate::catalog::Target::Panel("game".into()),
        )
    });
    settle(&mut rt);
    assert!(rt.find("tab/hierarchy").is_some(), "restored");
}

#[test]
fn the_play_menu_sets_mode_clients_network_and_start_paused() {
    let mut rt = open();
    settle(&mut rt);
    press(&mut rt, "play/options");
    assert!(rt.find("play/simulate").is_some(), "Simulate while stopped");
    assert!(rt.find("play/clients").is_none(), "Local has no clients");
    press(&mut rt, "play/mode/host");
    press(&mut rt, "play/clients/more");
    assert!(rt.find("play/network/2").is_some(), "a row per client");
    press(&mut rt, "play/network/1");
    press(&mut rt, "play/paused");
    let s = rt.state();
    assert_eq!(s.editor.state.settings["play.mode"], json!("host"));
    assert_eq!(s.editor.state.settings["play.clients"], json!(2.0));
    assert_eq!(crate::play_controls::network_profile(&s.editor, 1), "lan");
    assert_eq!(crate::play_controls::network_profile(&s.editor, 2), "clean");
    assert_eq!(s.editor.state.settings["play.startPaused"], json!(true));
    assert!(s.local.play_menu, "settings keep the menu open");
}

#[test]
fn import_defaults_set_and_reset_the_projects_importer_values() {
    let mut rt = open();
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "import"})));
    settle(&mut rt);
    let mipmaps = |rt: &Runtime<Studio>| {
        crate::import_edit::inherited_defaults(
            &rt.state().editor,
            "ara.texture",
            crate::import_edit::settings("ara.texture").unwrap().0,
        )
        .unwrap()["mipmaps"]
            .clone()
    };
    let before = mipmaps(&rt);
    assert!(
        rt.find("import/defaults/reset").is_some(),
        "the project's defaults show"
    );
    press(&mut rt, "asset/setting/mipmaps");
    assert_ne!(mipmaps(&rt), before, "the texture default changed");
    press(&mut rt, "import/defaults/reset");
    assert_eq!(mipmaps(&rt), before, "reset to built-in");
}

#[test]
fn the_computers_own_messages_fold_into_their_own_console_group() {
    let mut rt = open();
    let row = |seq: u64, diagnostic: ara_diag::Diagnostic| super::state::LogRow {
        seq,
        severity: ara_diag::Severity::Warning,
        code: "ARA".into(),
        message: diagnostic.message.clone(),
        origin: "engine".into(),
        at: 0,
        source: None,
        count: 1,
        diagnostic: std::sync::Arc::new(diagnostic),
    };
    rt.update(|s| {
        s.act("editor.layout.show", json!({"panel": "console"}));
        s.logs = vec![
            row(
                1,
                ara_diag::Diagnostic::warning(
                    ara_diag::codes::GPU_ENVIRONMENT,
                    "An overlay layer is old",
                ),
            ),
            row(
                2,
                ara_diag::Diagnostic::warning(crate::codes::DIAGNOSTIC_SAMPLE, "The project warns"),
            ),
        ];
        s.local.log_seen = 2;
    });
    settle(&mut rt);
    assert!(
        rt.find("console/machine").is_some(),
        "the computer's group shows"
    );
    assert!(rt.find("console/row/1").is_none(), "folded until opened");
    assert!(
        rt.find("console/row/2").is_some(),
        "the project's own message shows"
    );
    press(&mut rt, "console/machine");
    assert!(rt.find("console/row/1").is_some(), "opened");
}

#[test]
fn problems_filter_explain_and_go_to_what_they_name() {
    let mut rt = open();
    let mut diagnostic =
        ara_diag::Diagnostic::error(crate::codes::DIAGNOSTIC_SAMPLE, "Lamp is broken");
    diagnostic.location.record = Some("e_lamp".into());
    let row = |seq: u64, severity, diagnostic: ara_diag::Diagnostic| super::state::LogRow {
        seq,
        severity,
        code: diagnostic.code.to_string(),
        message: diagnostic.message.clone(),
        origin: "engine".into(),
        at: 0,
        source: Some("levels/main.scene".into()),
        count: 1,
        diagnostic: std::sync::Arc::new(diagnostic),
    };
    let warning = ara_diag::Diagnostic::warning(crate::codes::DIAGNOSTIC_SAMPLE, "Box is odd");
    rt.update(|s| {
        s.act("editor.layout.show", json!({"panel": "problems"}));
        s.logs = vec![
            row(1, ara_diag::Severity::Error, diagnostic),
            row(2, ara_diag::Severity::Warning, warning),
        ];
        s.local.log_seen = 2;
    });
    settle(&mut rt);
    assert!(rt.find("problems/1").is_some() && rt.find("problems/2").is_some());
    press(&mut rt, "problems/level/1");
    assert!(rt.find("problems/2").is_none(), "warnings hidden");
    rt.update(|s| s.local.problems_search = "nothing like it".into());
    settle(&mut rt);
    assert!(rt.find("problems/1").is_none());
    rt.update(|s| s.local.problems_search.clear());
    settle(&mut rt);
    press(&mut rt, "problems/1/why");
    assert!(rt.state().local.problems_open.contains(&1));
    press(&mut rt, "problems/1/go");
    assert_eq!(rt.state().editor.state.selection, vec!["e_lamp".to_owned()]);
}

#[test]
fn named_targets_run_as_agents_and_the_palette_send_them() {
    let mut rt = open();
    settle(&mut rt);
    use crate::catalog::Target;
    let run = |rt: &mut Runtime<Studio>, id: &str, target: Target| {
        rt.update(|s| super::commands::run(s, id, &target));
        settle(rt);
    };
    let failures = |rt: &mut Runtime<Studio>| {
        rt.update(|s| {
            s.poll_logs();
        });
        rt.state()
            .logs
            .iter()
            .filter(|r| r.severity == ara_diag::Severity::Error)
            .count()
    };
    let before = failures(&mut rt);
    run(
        &mut rt,
        "field.set",
        Target::Text("Transform.position=[1,2,3]".into()),
    );
    assert_eq!(position(&rt, "e_box"), json!([1, 2, 3]));
    let txn = rt
        .state()
        .editor
        .host
        .store()
        .history()
        .last()
        .unwrap()
        .id
        .0
        .clone();
    run(&mut rt, "snap.set", Target::Text("move:0.25".into()));
    assert_eq!(rt.state().editor.state.move_snap, 0.25);
    run(
        &mut rt,
        "edit.select_entity",
        Target::Entity("e_lamp".into()),
    );
    assert_eq!(rt.state().editor.state.selection, vec!["e_lamp".to_owned()]);
    run(&mut rt, "view.camera.fov.set", Target::Text("60°".into()));
    assert_eq!(rt.state().editor.state.camera.fov_y_degrees, 60.0);
    run(&mut rt, "console.errors", Target::None);
    assert!(!rt.state().local.console[0]);
    run(&mut rt, "view.tool.select", Target::None);
    assert!(rt.state().editor.state.hide_gizmos);
    // A transaction copies whole (History's Copy).
    run(&mut rt, "history.copy", Target::Text(txn.clone()));
    assert!(
        rt.state()
            .local
            .copy
            .as_deref()
            .is_some_and(|t| t.contains(&txn))
    );
    run(&mut rt, "window.minimize", Target::None);
    assert_eq!(
        rt.state().local.window,
        Some(super::state::WindowRequest::Minimize)
    );
    // View › Full Screen (F11) asks the window for full screen.
    run(&mut rt, "view.fullscreen", Target::None);
    assert_eq!(
        rt.state().local.window,
        Some(super::state::WindowRequest::Fullscreen)
    );
    assert_eq!(failures(&mut rt), before, "nothing failed");
}

#[test]
fn a_prefab_instance_lists_its_overrides_to_apply_or_revert() {
    let mut rt = open();
    rt.update(|s| {
        super::commands::run(
            s,
            "create.prefab",
            &crate::catalog::Target::Entity("e_box".into()),
        )
    });
    let instance = rt.state().editor.state.selection[0].clone();
    let overrides = |rt: &Runtime<Studio>| {
        let s = rt.state();
        crate::prefab::instance(s.editor.host.store(), &s.editor.state.doc, &instance)
            .expect("a prefab instance")
            .overrides
    };
    let scale = |rt: &mut Runtime<Studio>, v: f64| {
        let entity = rt.state().editor.state.selection[0].clone();
        rt.update(|s| s.apply(vec![json!({"cmd":"property.set","args":{"entity":entity,"path":"Transform.scale","value":[v,v,v]}})], "Scale"));
        settle(rt);
    };
    scale(&mut rt, 2.0);
    assert_eq!(overrides(&rt), vec!["Transform.scale".to_owned()]);
    assert!(
        rt.find("inspector/prefab").is_some(),
        "the prefab bar shows"
    );
    press(&mut rt, "inspector/overrides");
    press(&mut rt, "inspector/overrides/Transform.scale/apply");
    settle(&mut rt);
    assert!(overrides(&rt).is_empty(), "applied into the prefab");
    assert_eq!(
        rt.state().resolved[&instance].record["Transform"]["scale"],
        json!([2.0, 2.0, 2.0]),
        "the instance keeps the value, now from its prefab"
    );
    scale(&mut rt, 3.0);
    rt.update(|s| {
        super::commands::run(
            s,
            "prefab.revert_override",
            &crate::catalog::Target::Field("Transform.scale".into()),
        )
    });
    settle(&mut rt);
    assert!(overrides(&rt).is_empty(), "reverted");
    assert_eq!(
        rt.state().resolved[&instance].record["Transform"]["scale"],
        json!([2.0, 2.0, 2.0])
    );
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

/// Writes the studio's chrome (the Scene hole left empty) in both themes, with each bottom
/// panel, to `target/studio-captures/` for design review: `cargo test -p ara-editor --lib
/// studio::tests::captures -- --ignored`.
/// Gives the studio its Assets browser over a real scan of the test project.
fn scan(rt: &mut Runtime<Studio>) {
    rt.update(|s| {
        let store = s.editor.host.store();
        s.assets = Some(crate::assets::Browser::new(
            s.editor.host.root(),
            store.schema(),
            store.workspace().reference_root(),
        ));
    });
    for _ in 0..200 {
        if rt.touch(|s| s.assets.as_mut().is_some_and(crate::assets::Browser::poll)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn assets_save_searches_hide_support_files_and_move_and_import() {
    let mut rt = open();
    let root = rt.state().editor.host.root().to_owned();
    let content = root.join(ara_doc::AUTHORING_ROOT);
    std::fs::create_dir_all(content.join("art")).unwrap();
    std::fs::write(content.join("levels/notes.md"), "# Notes").unwrap();
    std::fs::write(content.join("art/keep.txt"), "").unwrap();
    scan(&mut rt);
    let storage = |rt: &Runtime<Studio>, name: &str| {
        rt.state()
            .assets
            .as_ref()
            .unwrap()
            .items
            .iter()
            .find(|a| a.path.ends_with(name))
            .map(|a| a.path.clone())
    };
    let levels = storage(&rt, "main.scene")
        .unwrap()
        .rsplit_once('/')
        .unwrap()
        .0
        .to_owned();
    rt.update(|s| s.act("editor.workspace", json!({"folder": levels})));
    settle(&mut rt);
    let notes = storage(&rt, "notes.md").unwrap();
    assert!(rt.find(&format!("asset/{notes}")).is_none(), "notes hide");
    press(&mut rt, "assets/menu");
    press(&mut rt, "assets/menu/all");
    assert!(
        rt.find(&format!("asset/{notes}")).is_some(),
        "Show All Files shows them"
    );
    // A search saves under its query, opens again and goes.
    rt.update(|s| s.act("editor.workspace", json!({"assetQuery": "main"})));
    settle(&mut rt);
    press(&mut rt, "assets/search/save");
    assert!(rt.find("assets/saved/main").is_some());
    press(&mut rt, "assets/saved/main/remove");
    assert!(rt.find("assets/saved/main").is_none());
    rt.update(|s| s.act("editor.workspace", json!({"assetQuery": ""})));
    // Move to Folder: pick the folder, the file moves.
    let art = storage(&rt, "keep.txt")
        .unwrap()
        .rsplit_once('/')
        .unwrap()
        .0
        .to_owned();
    rt.update(|s| {
        super::commands::run(
            s,
            "assets.move",
            &crate::catalog::Target::Asset(notes.clone()),
        )
    });
    settle(&mut rt);
    press(&mut rt, &format!("assets/move/{art}"));
    finish_file_jobs(&mut rt, &content.join("art/notes.md"));
    assert!(!content.join("levels/notes.md").exists());
    // Import a file from outside, then again: the second asks before overwriting.
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("readme.txt"), "hello").unwrap();
    let import = |rt: &mut Runtime<Studio>| {
        rt.update(|s| super::asset_ops::import(s, vec![outside.join("readme.txt")], Some(&art)));
        for _ in 0..3000 {
            if rt.touch(|s| {
                s.editor.tick();
                s.importer.poll(&mut s.editor);
                s.importer.progress().is_none()
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    import(&mut rt);
    assert!(
        content.join("art/readme.txt").is_file(),
        "{:?}",
        rt.state().importer.note
    );
    import(&mut rt);
    assert!(
        rt.state().importer.conflicts.is_some(),
        "an existing file asks first"
    );
    settle(&mut rt);
    press(&mut rt, "assets/conflict/skip");
    assert!(rt.state().importer.conflicts.is_none());
}

#[test]
fn scene_overlays_that_do_not_fit_collapse_to_icons_until_there_is_room() {
    let mut rt = open();
    let keys: Vec<String> = std::iter::once("scene/view".to_owned())
        .chain(
            super::scene_overlays::OVERLAYS
                .iter()
                .map(|d| format!("scene/overlay/{}", d.id)),
        )
        .collect();
    // A few passes of the host's feedback: rects reported, lengths measured.
    let report = |rt: &mut Runtime<Studio>| {
        for _ in 0..4 {
            settle(rt);
            rt.touch(|s| s.local.rects.clear());
            track(rt, &keys.iter().map(String::as_str).collect::<Vec<_>>());
            rt.update(super::scene_overlays::measure);
        }
    };
    // Top-edge overlays never overlap: their rects are disjoint along the edge.
    let disjoint = |rt: &Runtime<Studio>| {
        let mut top: Vec<_> = super::scene_overlays::OVERLAYS
            .iter()
            .filter(|d| d.id != "orientation")
            .filter_map(|d| rt.find(&format!("scene/overlay/{}", d.id)))
            .filter(|r| r.top < 140.0)
            .collect();
        top.sort_by(|a, b| a.left.total_cmp(&b.left));
        top.windows(2).all(|w| w[0].right <= w[1].left + 0.5)
    };
    report(&mut rt);
    assert!(disjoint(&rt), "wide: every toolbar fits");
    let wide = rt.find("scene/overlay/camera").unwrap().width();
    rt.resize((900, 900));
    report(&mut rt);
    assert!(
        disjoint(&rt),
        "narrow: toolbars squeeze rather than overlap"
    );
    let narrow: f32 = [
        "tools",
        "transform",
        "snapping",
        "view",
        "visibility",
        "camera",
    ]
    .iter()
    .filter_map(|id| rt.find(&format!("scene/overlay/{id}")))
    .map(|r| r.width())
    .sum();
    assert!(
        narrow < 900.0 - 300.0,
        "some collapsed to their icons: {narrow}"
    );
    rt.resize((1600, 900));
    report(&mut rt);
    let back = rt.find("scene/overlay/camera").unwrap().width();
    assert!(
        (back - wide).abs() < 1.0,
        "wide again: it expands ({wide} → {back})"
    );
    // A preset saves the layout; loading it puts a moved overlay back.
    use crate::catalog::Target;
    rt.update(|s| {
        super::commands::run(
            s,
            "view.overlays.preset.save",
            &Target::Text("Clean".into()),
        )
    });
    let before = rt
        .state()
        .editor
        .state
        .settings
        .get("viewport.overlays")
        .cloned();
    rt.update(|s| super::scene_overlays::update(s, "camera", |d| d.hidden = true));
    rt.update(|s| {
        super::commands::run(
            s,
            "view.overlays.preset.load",
            &Target::Text("Clean".into()),
        )
    });
    assert_eq!(
        rt.state()
            .editor
            .state
            .settings
            .get("viewport.overlays")
            .cloned(),
        before
    );
    assert!(!super::scene_overlays::dock(rt.state(), "camera").hidden);
    rt.update(|s| {
        super::commands::run(
            s,
            "view.overlays.preset.delete",
            &Target::Text("Clean".into()),
        )
    });
    assert!(super::scene_overlays::presets(rt.state()).is_empty());
}

#[test]
fn the_profiler_picks_the_editor_or_a_player_and_records_it() {
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "window.profiler", &crate::catalog::Target::None));
    settle(&mut rt);
    press(&mut rt, "profiler/target");
    assert!(
        rt.find("profiler/target/editor").is_some(),
        "the editor is a target"
    );
    rt.update(|s| s.local.select = None);
    settle(&mut rt);
    press(&mut rt, "profiler/record");
    assert!(
        rt.touch(|s| s.editor.host.profiler().is_recording()),
        "the editor records"
    );
    rt.update(|s| s.local.profiling = true);
    settle(&mut rt);
    press(&mut rt, "profiler/stop");
    assert!(!rt.touch(|s| s.editor.host.profiler().is_recording()));
    rt.update(|s| assert!(!super::profiler::poll(s, true) || s.local.profiler_target.is_none()));
    // A baseline freezes what shows, to compare later captures' scopes with it.
    rt.update(|s| s.profile = std::sync::Arc::new(sample_profile()));
    settle(&mut rt);
    press(&mut rt, "profiler/baseline");
    assert!(rt.state().editor.profile_baseline.is_some());
    press(&mut rt, "profiler/baseline/clear");
    assert!(rt.state().editor.profile_baseline.is_none());
}

#[test]
fn mesh_paint_sets_its_brush_and_sends_scene_presses_to_the_host() {
    use crate::catalog::Target;
    use crate::paint::PaintTool;
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "paint.mode", &Target::None));
    settle(&mut rt);
    rt.update(|s| {
        s.poll_logs();
    });
    assert!(rt.state().editor.state.paint.is_some(), "Mesh Paint is on");
    assert!(rt.find("paint/mode").is_some(), "its panel shows");
    press(&mut rt, "paint/tool/fill");
    press(&mut rt, "paint/paint.channel.alpha");
    press(&mut rt, "paint/paint.color.red");
    press(&mut rt, "paint/swap");
    let p = rt.state().editor.state.paint.unwrap();
    assert_eq!(p.tool, PaintTool::Fill);
    assert!(!p.channels[3], "alpha off");
    assert_eq!(p.erase, [1.0, 0.0, 0.0, 1.0], "red swapped to erase");
    rt.update(|s| super::commands::run(s, "paint.radius.up", &Target::None));
    assert!(rt.state().editor.state.paint.unwrap().radius > p.radius);
    // A press in the Scene goes to the host's painter, not to picking.
    let view = rt.find("scene/view").unwrap();
    rt.pointer_move(view.center().x, view.center().y);
    rt.frame(FRAME);
    rt.pointer_down(view.center().x, view.center().y);
    rt.frame(FRAME);
    assert!(matches!(
        rt.state().local.paint_input.first(),
        Some(super::state::PaintInput::Down(..))
    ));
    rt.pointer_up(view.center().x, view.center().y);
    rt.update(|s| super::commands::run(s, "paint.mode", &Target::None));
    assert!(rt.state().editor.state.paint.is_none(), "off again");
}

#[test]
fn lighting_applies_look_presets_to_a_volume() {
    use crate::catalog::Target;
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "create.light.probe", &Target::None));
    let probe = rt.state().editor.state.selection[0].clone();
    assert!(
        rt.state().resolved[&probe]
            .record
            .contains_key("ReflectionProbe")
    );
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "lighting"})));
    settle(&mut rt);
    // Recapture waits for a renderer that can capture probes (`recapturing_probes_says_...`).
    // A look preset appends override groups to the selected volume.
    let before = rt.state().resolved["e_post"].record["PostProcessVolume"].clone();
    rt.update(|s| s.select(vec!["e_post".into()], false));
    rt.update(|s| super::commands::run(s, "lighting.preset.manual", &Target::None));
    assert_ne!(
        rt.state().resolved["e_post"].record["PostProcessVolume"],
        before
    );
}

#[test]
fn history_searches_opens_changes_and_undoes_back_to_a_step() {
    let mut rt = open();
    let mut ids = Vec::new();
    for (x, why) in [(1, "Move east"), (2, "Move further"), (3, "Move again")] {
        rt.update(|s| s.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_box","path":"Transform.position","value":[x,0,0]}})], why));
        ids.push(
            rt.state()
                .editor
                .host
                .store()
                .history()
                .last()
                .unwrap()
                .id
                .0
                .clone(),
        );
    }
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "history"})));
    settle(&mut rt);
    rt.update(|s| s.local.history_search = "further".into());
    settle(&mut rt);
    assert!(rt.find(&format!("history/{}", ids[1])).is_some());
    assert!(
        rt.find(&format!("history/{}", ids[0])).is_none(),
        "searched out"
    );
    rt.update(|s| s.local.history_search.clear());
    settle(&mut rt);
    press(&mut rt, &format!("history/{}", ids[1]));
    assert!(
        rt.find(&format!("history/{}/copy", ids[1])).is_some(),
        "its changes open"
    );
    press(&mut rt, &format!("history/{}/undo-to", ids[0]));
    assert_eq!(
        position(&rt, "e_box"),
        json!([0, 0, 0]),
        "back before the first move"
    );
}

#[test]
fn play_clients_show_side_by_side_and_a_click_takes_control_of_one() {
    // Three players tile two by two; each tile is the view's quarter, 4 px apart.
    let tiles = super::game::tiles(3, [804.0, 604.0], 4.0);
    assert_eq!(tiles.len(), 3);
    assert_eq!(tiles[0], [0.0, 0.0, 400.0, 300.0]);
    assert_eq!(tiles[1], [404.0, 0.0, 400.0, 300.0]);
    assert_eq!(tiles[2], [0.0, 304.0, 400.0, 300.0]);
    let mut rt = open();
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "game"})));
    rt.update(|s| {
        s.editor.state.playing = Some(true);
        s.local.game.live = true;
        s.local.game.players = vec![(0, [0, 0, 300, 200], true), (1, [304, 0, 300, 200], true)];
    });
    settle(&mut rt);
    assert!(rt.find("game/tile/0").is_some() && rt.find("game/tile/1").is_some());
    press(&mut rt, "game/tile/1");
    assert_eq!(
        rt.state().play.client,
        1,
        "Client 1 takes the keyboard and mouse"
    );
    assert!(rt.state().local.game_captured);
}

#[test]
fn a_panel_floats_in_its_own_window_that_moves_and_docks_back() {
    use crate::catalog::Target;
    let mut rt = open();
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "console"})));
    settle(&mut rt);
    rt.update(|s| super::commands::run(s, "window.float", &Target::Panel("console".into())));
    settle(&mut rt);
    let floating = |rt: &Runtime<Studio>| rt.state().editor.state.floating.clone();
    let window = floating(&rt);
    assert_eq!(window.len(), 1, "Console floats in a window");
    let id = window[0].id.clone();
    assert!(
        rt.find("tab/console").is_some(),
        "its tab shows in the window"
    );
    // Its title bar moves it.
    let bar = rt.find(&format!("float/{id}/bar")).unwrap();
    let (x, y) = (bar.left + 30.0, bar.center().y);
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    rt.pointer_down(x, y);
    rt.pointer_move(x + 60.0, y + 40.0);
    rt.frame(FRAME);
    rt.pointer_up(x + 60.0, y + 40.0);
    settle(&mut rt);
    let moved = floating(&rt)[0].rect;
    assert_eq!(
        [moved[0] - window[0].rect[0], moved[1] - window[0].rect[1]],
        [60, 40],
        "moved by the drag"
    );
    // Its tab drags back into the Scene's strip: the window goes.
    let keys = super::dock::all_drag_keys(rt.state());
    track(
        &mut rt,
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let scene = super::dock::area_holding(rt.state(), "scene").unwrap();
    let strip = rt.find(&format!("area/{scene}")).unwrap();
    let tab = rt.find("tab/console").unwrap();
    rt.pointer_move(tab.center().x, tab.center().y);
    rt.frame(FRAME);
    rt.pointer_down(tab.center().x, tab.center().y);
    rt.pointer_move(tab.center().x + 20.0, tab.center().y + 20.0);
    rt.pointer_move(strip.right - 40.0, strip.top + 10.0);
    rt.frame(FRAME);
    rt.pointer_up(strip.right - 40.0, strip.top + 10.0);
    settle(&mut rt);
    assert!(floating(&rt).is_empty(), "the emptied window closes");
    assert_eq!(
        super::dock::area_holding(rt.state(), "console"),
        Some(scene)
    );
}

#[test]
fn a_native_floating_window_shows_its_region_and_its_tab_docks_back_or_tears_out() {
    use crate::catalog::Target;
    let mut rt = open();
    // The host's native windows: the main window at (200, 100) on the desktop.
    rt.update(|s| {
        s.local.desktop.on = true;
        s.local.desktop.origin = (200.0, 100.0);
        s.act("editor.layout.show", json!({"panel": "console"}));
    });
    settle(&mut rt);
    rt.update(|s| super::commands::run(s, "window.float", &Target::Panel("console".into())));
    settle(&mut rt);
    let floating = |rt: &Runtime<Studio>| rt.state().editor.state.floating.clone();
    let window = floating(&rt);
    assert_eq!(window.len(), 1, "Console floats");
    let id = window[0].id.clone();
    assert!(
        window[0].rect[0] >= 200 && window[0].rect[1] >= 100,
        "opened on the desktop over the main window: {:?}",
        window[0].rect
    );
    // It lays out in its region, beyond the main window, and shows there alone.
    let region = super::dock::region(rt.state(), 0, &id, window[0].rect);
    rt.set_portals(&[region]);
    settle(&mut rt);
    let bar = rt.find(&format!("float/{id}/bar")).expect("its title bar");
    let (w, _) = rt.state().size;
    assert!(
        bar.left >= w && bar.left >= region.left,
        "{bar:?} in {region:?}"
    );
    let image = rt.snap_region(region).unwrap();
    assert_eq!(
        (image.width, image.height),
        (region.width() as u32, region.height() as u32)
    );
    // Its tab drags back into the Scene's strip: the host sends the pointer to the main
    // window once the desktop point is over it, and the native window goes.
    let keys = super::dock::all_drag_keys(rt.state());
    track(
        &mut rt,
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let scene = super::dock::area_holding(rt.state(), "scene").unwrap();
    let strip = rt.find(&format!("area/{scene}")).unwrap();
    let tab = rt.find("tab/console").unwrap();
    assert!(tab.left >= region.left, "the tab is in the region");
    rt.pointer_move(tab.center().x, tab.center().y);
    rt.frame(FRAME);
    rt.pointer_down(tab.center().x, tab.center().y);
    rt.pointer_move(tab.center().x + 20.0, tab.center().y + 20.0);
    rt.pointer_move(strip.right - 40.0, strip.top + 10.0);
    rt.frame(FRAME);
    rt.pointer_up(strip.right - 40.0, strip.top + 10.0);
    settle(&mut rt);
    assert!(floating(&rt).is_empty(), "the emptied window closes");
    assert_eq!(
        super::dock::area_holding(rt.state(), "console"),
        Some(scene.clone())
    );
    // Its title bar's Dock button puts it back where the default layout keeps it.
    rt.update(|s| super::commands::run(s, "window.float", &Target::Panel("console".into())));
    settle(&mut rt);
    let id = floating(&rt)[0].id.clone();
    let region = super::dock::region(rt.state(), 0, &id, floating(&rt)[0].rect);
    rt.set_portals(&[region]);
    settle(&mut rt);
    press(&mut rt, &format!("float/{id}/dock"));
    assert!(floating(&rt).is_empty(), "docked back");
    assert_eq!(
        super::dock::area_holding(rt.state(), "console").as_deref(),
        Some("bottom")
    );
    // Dragged out past the main window's edge, the tab tears out where the pointer is on
    // the desktop (a second monitor's, say).
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "console"})));
    settle(&mut rt);
    let tab = rt.find("tab/console").unwrap();
    let (w, h) = rt.state().size;
    rt.pointer_move(tab.center().x, tab.center().y);
    rt.frame(FRAME);
    rt.pointer_down(tab.center().x, tab.center().y);
    rt.pointer_move(tab.center().x + 20.0, tab.center().y + 20.0);
    rt.pointer_move(w + 300.0, h / 2.0);
    rt.touch(|s| s.local.desktop.pointer = Some((2400.0, 500.0)));
    rt.frame(FRAME);
    rt.pointer_up(w + 300.0, h / 2.0);
    settle(&mut rt);
    let torn = floating(&rt);
    assert_eq!(torn.len(), 1, "a window of its own");
    assert_eq!(super::dock::tabs_of(&torn[0].root), ["console"]);
    let [x, y, ..] = torn[0].rect;
    assert!(
        (x - 2400).abs() < 100 && (y - 500).abs() < 40,
        "under the pointer: {:?}",
        torn[0].rect
    );
}

#[test]
fn a_desktop_point_maps_into_the_topmost_window_under_it() {
    use super::portals::{Placement, locate};
    let main = Placement {
        client: [0.0, 0.0, 1000.0, 800.0],
        origin: (0.0, 0.0),
        k: 1.0,
    };
    // A floating window on a 1x monitor beside a 2x runtime: its pixels count double.
    let float = Placement {
        client: [900.0, 100.0, 400.0, 300.0],
        origin: (100_000.0, 0.0),
        k: 2.0,
    };
    let windows = [float, main];
    assert_eq!(locate(&windows, (950.0, 150.0)), Some((100_100.0, 100.0)));
    assert_eq!(locate(&windows, (500.0, 500.0)), Some((500.0, 500.0)));
    assert_eq!(locate(&windows, (2000.0, 500.0)), None);
}

#[test]
fn a_tab_menu_adds_tabs_beside_it_and_the_open_tab_closes_from_its_cross() {
    let mut rt = open();
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "console"})));
    settle(&mut rt);
    let tab = rt.find("tab/console").unwrap();
    rt.right_click(tab.center().x, tab.center().y);
    settle(&mut rt);
    assert!(
        rt.find("cmd/window.maximize").is_some(),
        "the tab's menu opens"
    );
    press(&mut rt, "sub/Add Tab");
    press(&mut rt, "cmd/window.add_tab/Import");
    settle(&mut rt);
    let area = |rt: &Runtime<Studio>, panel: &str| super::dock::area_holding(rt.state(), panel);
    assert_eq!(
        area(&rt, "import"),
        area(&rt, "console"),
        "added beside Console"
    );
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "import"})));
    settle(&mut rt);
    press(&mut rt, "tab/import/close");
    assert!(area(&rt, "import").is_none(), "closed from its ×");
    assert!(
        rt.find("tab/scene/close").is_none(),
        "the Scene view has none"
    );
}

#[test]
fn tabs_drag_into_another_area_or_split_one_off_its_edge() {
    let mut rt = open();
    settle(&mut rt);
    let area_of = |rt: &Runtime<Studio>, panel: &str| {
        fn find(n: &ara_ui::docking::DockNode, panel: &str) -> Option<String> {
            match n {
                ara_ui::docking::DockNode::Area { id, tabs, .. } => {
                    tabs.iter().any(|t| t == panel).then(|| id.clone())
                }
                ara_ui::docking::DockNode::Split { first, second, .. } => {
                    find(first, panel).or_else(|| find(second, panel))
                }
            }
        }
        find(&rt.state().editor.state.layout, panel)
    };
    // Drags tab `panel` to window point `to`, the host reporting where areas lie.
    let drag = |rt: &mut Runtime<Studio>, panel: &str, to: (f32, f32)| {
        let keys = super::dock::drag_keys(&rt.state().editor.state.layout);
        track(rt, &keys.iter().map(String::as_str).collect::<Vec<_>>());
        let tab = rt.find(&format!("tab/{panel}")).unwrap();
        rt.pointer_move(tab.center().x, tab.center().y);
        rt.frame(FRAME);
        rt.pointer_down(tab.center().x, tab.center().y);
        rt.pointer_move(tab.center().x + 20.0, tab.center().y + 20.0);
        rt.pointer_move(to.0, to.1);
        rt.frame(FRAME);
        assert!(
            rt.find("dock/drop").is_some(),
            "the landing shows while dragging"
        );
        rt.pointer_up(to.0, to.1);
        settle(rt);
    };
    let scene = area_of(&rt, "scene").unwrap();
    let view = rt.find(&format!("area/{scene}")).unwrap();
    // Console onto the Scene's left edge: a new area left of the Scene.
    drag(&mut rt, "console", (view.left + 10.0, view.center().y));
    let console = area_of(&rt, "console").unwrap();
    assert_ne!(console, scene);
    let (c, v) = (
        rt.find(&format!("area/{console}")).unwrap(),
        rt.find(&format!("area/{scene}")).unwrap(),
    );
    assert!(
        c.right <= v.left + 1.0,
        "split off to the left: {c:?} {v:?}"
    );
    // Hierarchy into the Scene's tab strip: a tab beside the Scene.
    let strip = rt.find(&format!("area/{scene}")).unwrap();
    drag(&mut rt, "hierarchy", (strip.right - 40.0, strip.top + 10.0));
    assert_eq!(area_of(&rt, "hierarchy").as_deref(), Some(scene.as_str()));
    assert!(rt.state().local.tab_drag.is_none());
}

#[test]
fn menus_list_scenes_and_layouts_and_filters_and_layouts_save_by_name() {
    use crate::catalog::Target;
    let mut rt = open();
    settle(&mut rt);
    // Save Layout As asks for a name, then the Window menu lists it.
    rt.update(|s| super::commands::run(s, "layout.save", &Target::None));
    settle(&mut rt);
    rt.update(|s| {
        if let Some(p) = s.local.prompt.as_mut() {
            p.text = "Wide".into();
        }
    });
    settle(&mut rt);
    press(&mut rt, "prompt/ok");
    assert!(rt.state().local.prompt.is_none());
    assert!(
        crate::state::saved_layouts(rt.state().editor.host.root()).contains(&"Wide".to_owned())
    );
    press(&mut rt, "menu/Window");
    press(&mut rt, "sub/Layouts");
    assert!(
        rt.find("cmd/layout.load/Wide").is_some(),
        "Window › Layouts lists saved layouts"
    );
    press(&mut rt, "menu/Window");
    // Open Recent lists the project's scenes.
    press(&mut rt, "menu/File");
    press(&mut rt, "sub/Open Recent");
    assert!(
        rt.find("cmd/file.open_scene/levels/main.scene").is_some(),
        "File › Open Recent lists scenes"
    );
    rt.key(stillkit::el::KeyCode::ESCAPE.0);
    settle(&mut rt);
    // A Hierarchy filter saves, applies and goes.
    rt.update(|s| s.local.hierarchy_query = "t:light".into());
    settle(&mut rt);
    press(&mut rt, "hierarchy/filters");
    press(&mut rt, "hierarchy/filters/save");
    rt.update(|s| s.local.hierarchy_query.clear());
    settle(&mut rt);
    rt.update(|s| {
        super::commands::run(s, "hierarchy.filter.apply", &Target::Text("t:light".into()))
    });
    assert_eq!(rt.state().local.hierarchy_query, "t:light");
    rt.update(|s| {
        super::commands::run(
            s,
            "hierarchy.filter.remove",
            &Target::Text("t:light".into()),
        )
    });
    assert!(super::hierarchy::saved_filters(rt.state()).is_empty());
    // Why This Value? says where a value comes from.
    rt.update(|s| {
        super::commands::run(s, "field.why", &Target::Field("Transform.position".into()))
    });
    let note = rt
        .state()
        .local
        .note
        .clone()
        .map(|(n, _)| n)
        .unwrap_or_default();
    assert!(note.contains("Set on this entity"), "{note}");
    // About opens and closes.
    rt.update(|s| super::commands::run(s, "help.about", &Target::None));
    settle(&mut rt);
    press(&mut rt, "about/close");
    assert!(rt.state().local.about.is_none());
}

#[test]
fn new_script_is_named_in_place_then_compiles_and_attaches() {
    let mut rt = open();
    scan(&mut rt);
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
    settle(&mut rt);
    rt.update(|s| super::commands::run(s, "component.new_script", &crate::catalog::Target::None));
    settle(&mut rt);
    let (path, name) = rt
        .state()
        .local
        .asset_rename
        .clone()
        .expect("named in place");
    assert_eq!(name, "NewBehaviour", "the stem, without .cs");
    assert!(rt.find("asset/rename").is_some());
    rt.update(|s| super::asset_ops::rename(s, &path, "Mover"));
    let root = rt.state().editor.host.root().to_owned();
    let moved = path.replace("NewBehaviour", "Mover");
    finish_file_jobs(&mut rt, &root.join(&moved));
    let source = std::fs::read_to_string(root.join(&moved)).expect("renamed on disk");
    assert!(source.contains("class Mover"), "the class follows the file");
    // Where the engine's C# tools are built it compiles and attaches; elsewhere the
    // compile says why it could not.
    rt.update(|s| {
        s.resolve();
        s.poll_logs();
    });
    let s = rt.state();
    let attached = s.resolved["e_box"].record.get("Mover").is_some();
    let explained = s
        .logs
        .iter()
        .any(|r| r.message.starts_with("Compile Mover.cs: "));
    assert!(
        attached || explained,
        "compiled and attached, or said why not"
    );
}

#[test]
#[ignore = "writes review captures under target/"]
fn captures() {
    let mut rt = open();
    scan(&mut rt);
    rt.update(|s| {
        s.act(
            "editor.workspace",
            json!({"folder": ara_doc::AUTHORING_ROOT}),
        )
    });
    // A favourite and a saved search show above the folder tree.
    rt.update(|s| {
        s.act("editor.workspace", json!({"favourite": "", "favouriteFolder": true}));
        s.act("editor.workspace", json!({"saveSearch": "Rocks", "savedSearch": {"query": "rock", "folder": "", "folderOnly": false}}));
    });
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/studio-captures");
    std::fs::create_dir_all(&out).unwrap();
    // Every shot in both themes, at 1x and at 2x (`@2x` in the name).
    let design = rt.state().size;
    for (light, scale) in [(false, 1.0), (true, 1.0), (false, 2.0), (true, 2.0)] {
        rt.set_scale(scale);
        rt.resize(((design.0 * scale) as u32, (design.1 * scale) as u32));
        let theme = format!(
            "{}{}",
            if light { "light" } else { "dark" },
            if scale > 1.0 { "@2x" } else { "" }
        );
        rt.update(|s| s.editor.state.light = light);
        if rt.touch(Studio::sync_look) {
            let theme = rt.state().look.theme.clone();
            let background = rt.state().look.c.background.0;
            rt.set_theme(theme);
            rt.set_background(Some(background));
        }
        rt.update(|s| s.profile = std::sync::Arc::new(sample_profile()));
        rt.update(found_clients);
        rt.update(sample_logs);

        for panel in [
            "assets",
            "console",
            "problems",
            "agents",
            "history",
            "lighting",
            "profiler",
            "mesh-paint",
            "game",
            "build",
            "material",
            "vfx",
            "animation",
        ] {
            rt.update(|s| {
                super::commands::run(s, &format!("window.{panel}"), &crate::catalog::Target::None)
            });
            host_feedback(&mut rt);
            for _ in 0..30 {
                rt.frame(FRAME);
            }
            let name = format!("{theme}-{panel}.png");
            std::fs::write(
                out.join(name),
                ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
            )
            .unwrap();
        }
        // The Size over Life curve in the VFX Selection, a key picked (its handles shown).
        if !light && scale == 1.0 {
            rt.touch(|s| {
                let doc = s
                    .editor
                    .action(
                        "editor.resource.create",
                        json!({"path": "samples/capture.graph", "domain": "vfx"}),
                    )
                    .unwrap()["doc"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                s.act("editor.workspace", json!({"resource": doc}));
            });
        }
        rt.update(|s| super::commands::run(s, "window.vfx", &crate::catalog::Target::None));
        let effect = effect_at(&rt, "samples/capture.graph");
        if let Some((emitter, module, curve)) = effect.emitters.iter().find_map(|e| {
            e.stacks.update.iter().find_map(|m| match &m.module {
                ara_vfx::Module::SizeOverLife { curve } => {
                    Some((e.name.clone(), m.name.clone(), curve.clone()))
                }
                _ => None,
            })
        }) {
            let mut shaped = curve;
            shaped.keys[0].interpolation = ara_curve::Interpolation::Cubic;
            rt.update(|s| {
                s.editor.state.resource_item = Some(format!("{emitter}|update|{module}"));
                s.local.curve_pick = Some((format!("curve/vfx/{emitter}/update/{module}"), 1));
                s.local.curve_drag = Some(super::curve::CurveDrag {
                    field: format!("curve/vfx/{emitter}/update/{module}"),
                    frame: super::curve::Frame::of(&shaped, Some((0.0, 1.0))),
                    curve: shaped,
                    grab: super::curve::Grab::Key(1),
                });
            });
            host_feedback(&mut rt);
            for _ in 0..30 {
                rt.frame(FRAME);
            }
            let name = format!("{theme}-vfx-curve.png");
            std::fs::write(
                out.join(name),
                ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
            )
            .unwrap();
            rt.update(|s| s.local.curve_drag = None);
        }
        // Scene overlays floating: Tools as a titled panel, Snapping as a column.
        rt.update(|s| {
            s.act("editor.layout.show", json!({"panel": "scene"}));
            s.act(
                "editor.settings.set",
                json!({"key": "viewport.overlays", "value": {
                    "tools": "float:120:140:panel",
                    "snapping": "float:420:120:vertical",
                }}),
            );
        });
        host_feedback(&mut rt);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        std::fs::write(
            out.join(format!("{theme}-scene-overlays-float.png")),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(super::scene_overlays::reset);
        // The Rect tool round the box (bounds as the host publishes them), its right edge lit.
        rt.update(|s| {
            super::commands::run(s, "view.tool.rect", &crate::catalog::Target::None);
            s.local.rect_bounds = Some(super::rect_tool::RectBounds {
                rotation: glam::Quat::IDENTITY,
                min: glam::Vec3::splat(-0.5),
                max: glam::Vec3::splat(0.5),
            });
            s.local.rect_hover = Some(super::rect_tool::Grip { u: 1, v: 0 });
        });
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        std::fs::write(
            out.join(format!("{theme}-scene-rect.png")),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| {
            super::commands::run(s, "view.tool.move", &crate::catalog::Target::None);
            s.local.rect_hover = None;
        });
        // A hovered Hierarchy row, its eye and lock offered.
        let lamp = rt.find("row/e_lamp").unwrap();
        rt.pointer_move(lamp.center().x + 40.0, lamp.center().y);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-hierarchy-hover.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        // The menu bar's menus.
        for menu in ["File", "Edit", "View", "Window", "Help"] {
            rt.update(|s| s.local.menu = Some(menu));
            for _ in 0..30 {
                rt.frame(FRAME);
            }
            let name = format!("{theme}-menu-{}.png", menu.to_lowercase());
            std::fs::write(
                out.join(name),
                ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
            )
            .unwrap();
        }
        // A submenu open beside its menu.
        rt.update(|s| {
            s.local.menu = Some("File");
            s.local.submenu = Some("Build & Export");
        });
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-menu-cascade.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| {
            s.local.menu = None;
            s.local.submenu = None;
        });
        // A Hierarchy search, its match marked in each name.
        rt.update(|s| s.local.hierarchy_query = "o".into());
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-hierarchy-search.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.hierarchy_query.clear());
        // The Profiler's call tree, one scope open.
        rt.update(|s| {
            super::commands::run(s, "window.profiler", &crate::catalog::Target::None);
            s.local.profiler_tree = true;
            s.local.profiler_open.insert("studio.scene".into());
        });
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-profiler-tree.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.profiler_tree = false);
        // The Inspector's Debug mode: the record itself.
        rt.update(|s| {
            s.act("editor.layout.show", json!({"panel": "inspector"}));
            s.local.inspector_raw = true;
        });
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-inspector-debug.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.inspector_raw = false);
        // The scene header's own menu.
        let header = rt.find("hierarchy/scene").unwrap();
        rt.right_click(header.center().x, header.center().y);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-scene-menu.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.context = None);
        settle(&mut rt);
        let row = rt.find("row/e_box").unwrap();
        rt.right_click(row.center().x, row.center().y);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-context.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.key(stillkit::el::KeyCode::ESCAPE.0);
        // The Play menu, as host with two clients.
        rt.update(|s| {
            for id in ["play.mode.host", "play.clients.more"] {
                super::commands::run(s, id, &crate::catalog::Target::None);
            }
            s.local.play_menu = true;
        });
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-play.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.play_menu = false);
        // A tab mid-drag onto the Scene's right edge.
        let keys = super::dock::drag_keys(&rt.state().editor.state.layout);
        track(
            &mut rt,
            &keys.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        if let Some(view) = rt.find("area/view") {
            let at = (view.right - 30.0, view.center().y);
            rt.update(|s| s.local.tab_drag = Some(("console".into(), at)));
            for _ in 0..10 {
                rt.frame(FRAME);
            }
            let name = format!("{theme}-tabdrag.png");
            std::fs::write(
                out.join(name),
                ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
            )
            .unwrap();
            rt.update(|s| s.local.tab_drag = None);
        }
        // The Profiler comparing a capture with its baseline.
        rt.update(|s| {
            super::commands::run(s, "window.profiler", &crate::catalog::Target::None);
            s.editor.profile_baseline = Some(std::sync::Arc::new(sample_profile()));
            let mut now = sample_profile();
            for frame in &mut now.frames {
                for span in &mut frame.spans {
                    span.self_ns = if span.label % 2 == 0 {
                        span.self_ns * 3 / 2
                    } else {
                        span.self_ns / 2
                    };
                }
            }
            s.profile = std::sync::Arc::new(now);
        });
        for _ in 0..10 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-profiler-compare.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| {
            s.editor.profile_baseline = None;
            s.profile = std::sync::Arc::new(sample_profile());
        });
        // A panel floating in its own window.
        rt.update(|s| {
            s.act("editor.layout.show", json!({"panel": "history"}));
            super::commands::run(
                s,
                "window.float",
                &crate::catalog::Target::Panel("history".into()),
            )
        });
        for _ in 0..10 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-floating.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| {
            super::commands::run(
                s,
                "window.move.bottom",
                &crate::catalog::Target::Panel("history".into()),
            )
        });
        rt.update(|s| {
            super::create::toggle(s, super::create::Anchor::Key("hierarchy/create"), None)
        });
        settle(&mut rt);
        rt.key(0x28);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-create.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.create = None);
        rt.update(|s| super::commands::run(s, "edit.preferences", &crate::catalog::Target::None));
        settle(&mut rt);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-settings.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.settings = None);
        rt.update(|s| super::commands::run(s, "edit.palette", &crate::catalog::Target::None));
        settle(&mut rt);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-palette.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        // A search reaches settings too.
        rt.update(|s| s.local.palette = Some(("camera".into(), 0)));
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-palette-search.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.palette = None);
        // The Inspector in front of Mesh Paint, as a person picking a component has it.
        rt.update(|s| s.act("editor.layout.show", json!({"panel": "inspector"})));
        rt.update(|s| s.local.add_component = Some(Default::default()));
        settle(&mut rt);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-add-component.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.add_component = None);
        // An asset in the Inspector: the open scene's file.
        rt.update(|s| {
            let path = s
                .assets
                .as_ref()
                .and_then(|b| b.items.iter().find(|a| a.path.ends_with(".scene")))
                .map(|a| a.path.clone());
            if let Some(path) = path {
                s.act("editor.workspace", json!({"asset": path}));
                s.local.inspect_asset = true;
            }
        });
        settle(&mut rt);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{theme}-asset-inspector.png");
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        rt.update(|s| s.local.inspect_asset = false);
    }
}

/// A recorded-looking profile: two threads of nested scopes, GPU passes, one slow frame.
fn sample_profile() -> ara_profile::Profile {
    use ara_profile::{CpuSpan, Frame, GpuPass, Profile, ThreadInfo};
    let labels: Vec<String> = [
        "studio.events",
        "studio.authority",
        "studio.scene",
        "studio.composite",
        "scene.render",
        "probe.gi",
        "Shadow pass",
        "GBuffer",
        "Lighting",
        "Post",
    ]
    .map(str::to_owned)
    .to_vec();
    let span = |label, thread, id, parent, start: u64, duration: u64, own: u64| CpuSpan {
        label,
        thread,
        id,
        parent,
        start_ns: start,
        duration_ns: duration,
        self_ns: own,
        allocated_bytes: own / 100,
        self_allocated_bytes: own / 100,
    };
    let frames = (1..=90u64)
        .map(|i| {
            let start = i * 20_000_000;
            let long = if i % 37 == 0 { 3 } else { 1 };
            let d = (9_000_000 + (i % 7) * 900_000) * long;
            Frame {
                id: i,
                start_ns: start,
                duration_ns: d,
                spans: vec![
                    span(0, 0, i * 10 + 1, 0, start, d / 5, d / 5),
                    span(1, 0, i * 10 + 2, 0, start + d / 5, d / 6, d / 6),
                    span(2, 0, i * 10 + 3, 0, start + d / 2, d / 3, d / 12),
                    span(4, 0, i * 10 + 4, i * 10 + 3, start + d / 2, d / 4, d / 4),
                    span(3, 0, i * 10 + 5, 0, start + 5 * d / 6, d / 7, d / 7),
                    span(5, 1, i * 10 + 6, 0, start + d / 10, d / 2, d / 2),
                ],
                gpu: [(6, 1_200_000), (7, 2_100_000), (8, 3_000_000), (9, 900_000)]
                    .map(|(label, duration_ns)| GpuPass {
                        label,
                        queue: 0,
                        duration_ns,
                    })
                    .to_vec(),
                // A heap growing 64 KB a frame, as a counting allocator reports it.
                memory: ara_profile::Memory {
                    cpu_heap: 180 * 1024 * 1024 + i * 64 * 1024,
                    allocated_bytes: 96 * 1024,
                    allocations: 140,
                    gpu_textures: 412 * 1024 * 1024,
                    gpu_buffers: 96 * 1024 * 1024,
                    gpu_acceleration: 24 * 1024 * 1024,
                    gpu_other: 8 * 1024 * 1024,
                },
                ..Default::default()
            }
        })
        .collect();
    Profile {
        version: 1,
        budget_ns: 16_666_667,
        labels,
        threads: vec![
            ThreadInfo {
                id: 0,
                name: "editor".into(),
            },
            ThreadInfo {
                id: 1,
                name: "probe-gi".into(),
            },
        ],
        frames,
        orphaned_results: 0,
    }
}

#[test]
fn arrow_keys_walk_the_hierarchy() {
    let mut rt = open();
    let row = rt.find("row/e_box").unwrap();
    rt.pointer_down(row.center().x, row.center().y);
    rt.pointer_up(row.center().x, row.center().y);
    rt.frame(FRAME);
    rt.key(0x28);
    rt.frame(FRAME);
    assert_eq!(
        rt.state().editor.state.selection,
        vec!["e_lamp".to_owned()],
        "Down moves to the next row"
    );
    rt.key(0x24);
    rt.frame(FRAME);
    assert_eq!(
        rt.state().editor.state.selection,
        vec!["e_box".to_owned()],
        "Home goes to the first row"
    );
}

#[test]
fn right_clicking_a_row_opens_its_entity_menu() {
    let mut rt = open();
    let row = rt.find("row/e_lamp").unwrap();
    rt.right_click(row.center().x, row.center().y);
    rt.frame(FRAME);
    assert_eq!(
        rt.state().editor.state.selection,
        vec!["e_lamp".to_owned()],
        "the row becomes the selection"
    );
    assert!(
        rt.find("cmd/edit.duplicate").is_some(),
        "the Entity menu opens"
    );
}

#[test]
fn the_create_browser_searches_and_creates_with_enter() {
    let mut rt = open();
    let plus = rt.find("hierarchy/create").unwrap();
    rt.pointer_down(plus.center().x, plus.center().y);
    rt.pointer_up(plus.center().x, plus.center().y);
    settle(&mut rt);
    assert!(rt.find("create").is_some(), "the browser opens");
    assert!(rt.find("create/cat/Lights").is_some(), "with categories");
    rt.type_text("spot");
    rt.frame(FRAME);
    assert!(
        rt.find("create/item/create.light.spot").is_some(),
        "search finds the spot light"
    );
    rt.key(0x0D);
    rt.frame(FRAME);
    assert!(
        rt.state().local.create.is_none(),
        "Enter creates and closes"
    );
    let selected = rt.state().editor.state.selection.clone();
    assert_eq!(selected.len(), 1);
    let created = &rt.state().resolved[&selected[0]].record;
    assert_eq!(created["Light"]["kind"], "spot");
    assert_eq!(rt.state().local.recent, vec!["create.light.spot"]);
}

#[test]
fn dragging_a_row_onto_another_makes_it_a_child() {
    let mut rt = open();
    let lamp = rt.find("row/e_lamp").unwrap();
    let boxed = rt.find("row/e_box").unwrap();
    rt.pointer_down(lamp.center().x, lamp.center().y);
    for step in 1..=4 {
        let y = lamp.center().y + (boxed.center().y - lamp.center().y) * step as f32 / 4.0;
        rt.pointer_move(lamp.center().x, y);
        rt.frame(FRAME);
    }
    assert!(
        rt.state()
            .local
            .row_drag
            .as_ref()
            .and_then(|d| d.target.as_ref())
            .is_some_and(|(t, _)| t == "e_box"),
        "the box is the target"
    );
    rt.pointer_up(lamp.center().x, boxed.center().y);
    rt.frame(FRAME);
    assert_eq!(
        rt.state().resolved["e_lamp"].parent.as_deref(),
        Some("e_box"),
        "the lamp moved inside the box"
    );
}

#[test]
fn a_fields_menu_copies_its_path() {
    let mut rt = open();
    let label = rt.find("field/Transform.position/0").unwrap();
    // The row's label column, left of the value cells.
    rt.right_click(label.left - 40.0, label.center().y);
    rt.frame(FRAME);
    let item = rt
        .find("field/Transform.position/path")
        .expect("the field's menu opens");
    rt.pointer_down(item.center().x, item.center().y);
    rt.pointer_up(item.center().x, item.center().y);
    rt.frame(FRAME);
    assert_eq!(
        rt.state().local.copy.as_deref(),
        Some("@levels/main.scene#e_box/Transform.position")
    );
}

#[test]
fn preferences_toggle_a_setting_and_reset_it() {
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "edit.preferences", &crate::catalog::Target::None));
    settle(&mut rt);
    assert!(rt.find("settings").is_some(), "the window opens");
    let def = crate::settings::DEFS
        .iter()
        .find(|d| {
            d.scope == crate::settings::Scope::User
                && matches!(d.kind, crate::settings::Kind::Toggle)
        })
        .expect("a user toggle");
    rt.update(|s| s.local.settings.as_mut().unwrap().query = def.label.to_owned());
    settle(&mut rt);
    let before = rt
        .state()
        .editor
        .state
        .settings
        .get(def.key)
        .cloned()
        .unwrap_or_else(|| def.default.value());
    let switch = rt
        .find(&format!("settings/{}/switch", def.key))
        .expect("its switch shows");
    rt.pointer_down(switch.center().x, switch.center().y);
    rt.pointer_up(switch.center().x, switch.center().y);
    settle(&mut rt);
    let after = rt.state().editor.state.settings.get(def.key).cloned();
    assert_eq!(
        after,
        Some(json!(!before.as_bool().unwrap())),
        "{} flipped",
        def.key
    );
    let reset = rt
        .find(&format!("settings/{}/reset", def.key))
        .expect("Reset shows once changed");
    rt.pointer_down(reset.center().x, reset.center().y);
    rt.pointer_up(reset.center().x, reset.center().y);
    settle(&mut rt);
    assert_eq!(
        rt.state()
            .editor
            .state
            .settings
            .get(def.key)
            .cloned()
            .unwrap_or_else(|| def.default.value()),
        def.default.value()
    );
}

#[test]
fn window_menu_layouts_and_panels() {
    let mut rt = open();
    let before = format!("{:?}", rt.state().editor.state.layout);
    rt.update(|s| super::commands::run(s, "layout.level_design", &crate::catalog::Target::None));
    settle(&mut rt);
    assert_ne!(
        format!("{:?}", rt.state().editor.state.layout),
        before,
        "the preset applies"
    );
    rt.update(|s| super::commands::run(s, "window.history", &crate::catalog::Target::None));
    settle(&mut rt);
    assert!(rt.find("tab/history").is_some(), "History opens as a tab");
}

#[test]
fn a_linked_scale_keeps_its_proportions() {
    let mut rt = open();
    rt.update(|s| s.local.scale_linked = true);
    rt.update(|s| super::inspector::commit_number(s, "Transform.scale", Some(1), 2.0));
    assert_eq!(
        rt.state().resolved["e_box"].record["Transform"]["scale"],
        json!([2.0, 2.0, 2.0])
    );
    rt.update(|s| s.local.scale_linked = false);
    rt.update(|s| super::inspector::commit_number(s, "Transform.scale", Some(0), 1.0));
    assert_eq!(
        rt.state().resolved["e_box"].record["Transform"]["scale"],
        json!([1.0, 2.0, 2.0])
    );
}

#[test]
fn scrubbing_a_linked_scale_previews_every_axis_before_it_commits() {
    let mut rt = open();
    rt.update(|s| s.local.scale_linked = true);
    settle(&mut rt);
    let before = rt.state().editor.host.store().revision();
    let tag = rt.find("field/Transform.scale/1/scrub").unwrap();
    let (x, y) = (tag.center().x, tag.center().y);
    rt.pointer_down(x, y);
    rt.frame(FRAME);
    // 8 px a step of 0.01: 40 px is +0.05 on Y, and the others follow.
    rt.pointer_move(x + 40.0, y);
    rt.frame(FRAME);
    let draft = rt.state().local.draft.clone().unwrap();
    assert_eq!(draft.0, "Transform.scale");
    assert_eq!(
        draft.1,
        json!([1.05, 1.05, 1.05]),
        "every axis moves with the drag"
    );
    assert_eq!(
        rt.state().editor.host.store().revision(),
        before,
        "nothing commits mid-drag"
    );
    rt.pointer_up(x + 40.0, y);
    rt.frame(FRAME);
    assert!(rt.state().local.draft.is_none());
    assert_eq!(
        rt.state().resolved["e_box"].record["Transform"]["scale"],
        json!([1.05, 1.05, 1.05])
    );
}

#[test]
fn the_animation_page_shows_states_added_to_its_controller() {
    let mut rt = open();
    let doc = rt.touch(|s| {
        let result = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/walk.animation", "domain": "animation"}),
            )
            .unwrap();
        let doc = result["doc"].as_str().unwrap().to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
        for _ in 0..2 {
            crate::animation_edit::edit(&mut s.editor, &doc, "state").unwrap();
        }
        doc
    });
    rt.touch(|s| super::graph::sync(s, "animation"));
    let nodes = super::graph::node_ids(rt.state());
    assert_eq!(nodes.len(), 2, "{doc}: {nodes:?}");
}

/// Records where `keys` are laid out, as the host does after each frame.
/// The host's layout feedback for a few passes: the Scene view's and overlays' rects
/// reported, overlay lengths measured (so overflow folds overlays as the app does).
fn host_feedback(rt: &mut Runtime<Studio>) {
    let keys: Vec<String> = std::iter::once("scene/view".to_owned())
        .chain(
            super::scene_overlays::OVERLAYS
                .iter()
                .map(|d| format!("scene/overlay/{}", d.id)),
        )
        .collect();
    for _ in 0..4 {
        settle(rt);
        rt.touch(|s| s.local.rects.clear());
        track(rt, &keys.iter().map(String::as_str).collect::<Vec<_>>());
        rt.update(super::scene_overlays::measure);
    }
}

fn track(rt: &mut Runtime<Studio>, keys: &[&str]) {
    let rects: Vec<_> = keys
        .iter()
        .filter_map(|k| rt.find(k).map(|r| ((*k).to_owned(), r)))
        .collect();
    rt.touch(|s| s.local.rects.extend(rects));
}

/// Waits for the file jobs an operation just queued to end and for `file` to exist: their
/// own completion, never an empty job list (which an operation that queued nothing also
/// shows). Fails at once when nothing was queued, and after a minute otherwise.
fn finish_file_jobs(rt: &mut Runtime<Studio>, file: &std::path::Path) {
    let jobs: Vec<String> = rt
        .state()
        .local
        .asset_jobs
        .iter()
        .map(|(job, _)| job.clone())
        .collect();
    assert!(
        !jobs.is_empty(),
        "the operation queued no file job for {}",
        file.display()
    );
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(60) {
        rt.touch(|s| {
            s.editor.tick();
            super::asset_ops::poll_jobs(s);
        });
        let running = |rt: &Runtime<Studio>| {
            rt.state()
                .local
                .asset_jobs
                .iter()
                .any(|(job, _)| jobs.contains(job))
        };
        if !running(rt) && file.is_file() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "{} after a minute; jobs {jobs:?} running: {:?}",
        file.display(),
        rt.state().local.asset_jobs
    );
}

/// Presses the element keyed `key` (hovered first, as a pointer would).
fn press(rt: &mut Runtime<Studio>, key: &str) {
    let r = rt.find(key).unwrap_or_else(|| panic!("{key} shows"));
    rt.pointer_move(r.center().x, r.center().y);
    rt.frame(FRAME);
    rt.pointer_down(r.center().x, r.center().y);
    rt.pointer_up(r.center().x, r.center().y);
    settle(rt);
}

#[test]
fn the_animator_adds_parameters_and_conditions_to_a_picked_transition() {
    let mut rt = open();
    let doc = rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/run.animation", "domain": "animation"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
        for kind in ["state", "state", "transition"] {
            crate::animation_edit::edit(&mut s.editor, &doc, kind).unwrap();
        }
        doc
    });
    rt.update(|s| super::commands::run(s, "window.animation", &crate::catalog::Target::None));
    rt.update(|s| super::graph::sync(s, "animation"));
    settle(&mut rt);
    let controller = |rt: &Runtime<Studio>| {
        let workspace = rt.state().editor.host.store().workspace();
        crate::authoring::animation::controller(workspace.doc(&doc).expect("doc"))
            .expect("controller")
            .1
    };
    press(&mut rt, "animation/param/add");
    press(&mut rt, "animation/param/add/Float");
    rt.update(|s| super::graph::sync(s, "animation"));
    assert!(
        controller(&rt).parameters.contains_key("Float1"),
        "Add made Float1"
    );
    let transition = controller(&rt).transitions[0].id.clone();
    rt.update(|s| {
        s.graph
            .pick(crate::authoring::animation::transition_edge(&transition))
    });
    settle(&mut rt);
    press(&mut rt, "animation/condition/add");
    rt.update(|s| super::graph::sync(s, "animation"));
    settle(&mut rt);
    let condition = controller(&rt).transitions[0].conditions[0].clone();
    assert_eq!(condition.parameter, "Float1");
    assert_eq!(condition.comparison, ara_anim::graph::Comparison::Greater);
    assert!(
        rt.find("animation/condition/0/value").is_some(),
        "its threshold is typed in place"
    );
}

#[test]
fn delete_removes_picked_states_and_transitions_but_keeps_the_entry() {
    let mut rt = open();
    let doc = rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/walk.animation", "domain": "animation"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
        for kind in ["state", "state", "transition"] {
            crate::animation_edit::edit(&mut s.editor, &doc, kind).unwrap();
        }
        doc
    });
    rt.update(|s| super::commands::run(s, "window.animation", &crate::catalog::Target::None));
    rt.update(|s| super::graph::sync(s, "animation"));
    settle(&mut rt);
    let controller = |rt: &Runtime<Studio>| {
        let workspace = rt.state().editor.host.store().workspace();
        crate::authoring::animation::controller(workspace.doc(&doc).expect("doc"))
            .expect("controller")
            .1
    };
    let before = controller(&rt);
    let transition = before.transitions[0].clone();
    rt.update(|s| {
        s.graph
            .pick(crate::authoring::animation::transition_edge(&transition.id));
        super::animator::delete_selection(s);
        super::graph::sync(s, "animation");
    });
    assert!(
        controller(&rt).transitions.is_empty(),
        "the transition went"
    );
    // A state other than the entry goes; the entry is refused.
    let node = |rt: &Runtime<Studio>, state: &str| {
        let graph = rt.state().graph.shown_graph().unwrap();
        graph
            .nodes
            .iter()
            .find(|(_, n)| n.payload["state"] == state)
            .map(|(id, _)| id.clone())
            .unwrap()
    };
    let entry = before.initial.clone();
    let other = before.states.keys().find(|k| **k != entry).unwrap().clone();
    let id = node(&rt, &other);
    rt.update(|s| {
        s.graph.pick(id);
        super::animator::delete_selection(s);
        super::graph::sync(s, "animation");
    });
    assert!(!controller(&rt).states.contains_key(&other), "{other} went");
    let id = node(&rt, &entry);
    rt.update(|s| {
        s.graph.pick(id);
        super::animator::delete_selection(s);
        super::graph::sync(s, "animation");
    });
    assert!(
        controller(&rt).states.contains_key(&entry),
        "the entry stays"
    );
}

#[test]
fn right_clicking_an_asset_offers_its_actions_and_a_press_outside_closes_them() {
    let mut rt = open();
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
    scan(&mut rt);
    rt.update(|s| {
        s.act(
            "editor.workspace",
            json!({"folder": ara_doc::AUTHORING_ROOT}),
        )
    });
    settle(&mut rt);
    let tile = rt
        .find("asset/content/levels")
        .expect("the levels folder shows");
    rt.right_click(tile.center().x, tile.center().y);
    settle(&mut rt);
    for id in [
        "assets.open",
        "assets.reveal",
        "assets.rename",
        "assets.duplicate",
        "assets.delete",
    ] {
        assert!(rt.find(&format!("cmd/{id}")).is_some(), "{id} is offered");
    }
    rt.pointer_down(5.0, 880.0);
    rt.pointer_up(5.0, 880.0);
    settle(&mut rt);
    assert!(
        rt.state().local.context.is_none(),
        "a press outside closes the menu"
    );
}

#[test]
fn right_clicking_the_material_graph_adds_a_node_where_it_was_clicked() {
    let mut rt = open();
    rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/look.graph", "domain": "material"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
    });
    rt.update(|s| super::commands::run(s, "window.material", &crate::catalog::Target::None));
    rt.touch(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    let before = super::graph::node_ids(rt.state()).len();
    let canvas = rt.find("graph/view").expect("the canvas shows");
    // Empty canvas near its bottom-left corner.
    let (x, y) = (canvas.left + 30.0, canvas.bottom - 30.0);
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    rt.right_click(x, y);
    settle(&mut rt);
    assert!(
        rt.state().graph.adding.is_some(),
        "the node search opens at the pointer"
    );
    let item = rt
        .find("material/add/material.add")
        .expect("the search lists Add");
    rt.pointer_move(item.center().x, item.center().y);
    rt.frame(FRAME);
    rt.pointer_down(item.center().x, item.center().y);
    rt.pointer_up(item.center().x, item.center().y);
    // The pick runs on the next frame; the host then loads the changed graph.
    settle(&mut rt);
    rt.touch(|s| super::graph::sync(s, "material"));
    let after = super::graph::node_ids(rt.state()).len();
    assert!(
        after > before,
        "Add and its default inputs land: {before} -> {after}"
    );
    assert!(rt.state().graph.adding.is_none());
}

#[test]
fn material_inputs_show_their_defaults_inline_and_edit_in_place() {
    let mut rt = open();
    rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/inline.graph", "domain": "material"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc.clone()}));
        s.editor
            .apply_to(
                &doc,
                vec![json!({"cmd": "editor.material.node.add", "args": {"kind": "material.add", "position": [40, 40]}})],
                "Add",
            )
            .unwrap();
    });
    rt.update(|s| super::commands::run(s, "window.material", &crate::catalog::Target::None));
    rt.touch(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    let values = super::graph::inline_values(rt.state());
    assert_eq!(
        values.len(),
        2,
        "Add's two inputs show their defaults: {values:?}"
    );
    let nodes = super::graph::node_ids(rt.state());
    assert!(
        values.iter().all(|(id, _)| !nodes.contains(id)),
        "the defaults are not nodes of their own"
    );
    let canvas = rt.find("graph/view").expect("the canvas shows");
    let (constant, (x, y)) = values[0].clone();
    let (x, y) = (canvas.left + x, canvas.top + y);
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert!(
        rt.state().graph.editing.is_some(),
        "a click opens the value"
    );
    // The whole value is selected: typing replaces it.
    rt.type_text("0.25");
    rt.key(0x0D);
    settle(&mut rt);
    rt.touch(|s| super::graph::sync(s, "material"));
    assert!(
        rt.state().graph.editing.is_none(),
        "Enter commits and closes"
    );
    let workspace = rt.state().editor.host.store().workspace();
    let doc = workspace
        .find("samples/inline.graph")
        .expect("the material");
    let graph =
        ara_graph::Graph::from_document(workspace.doc(doc.as_str()).expect("doc")).expect("graph");
    assert_eq!(graph.nodes[&constant].params["value"], json!([0.25]));
}

#[test]
fn the_material_source_view_edits_the_graph_as_text_and_wires_reroute() {
    let mut rt = open();
    let doc = rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/text.graph", "domain": "material"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc.clone()}));
        doc
    });
    rt.update(|s| super::commands::run(s, "window.material", &crate::catalog::Target::None));
    rt.touch(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    press(&mut rt, "material/view/source");
    assert!(rt.find("material/source").is_some(), "the text shows");
    assert!(
        !rt.state()
            .graph
            .source
            .clone()
            .unwrap_or_default()
            .is_empty()
    );
    // Typed text applies to the document; the graph follows.
    let showcase = SHOWCASE.to_owned();
    rt.update(|s| s.graph.source = Some(showcase.clone()));
    settle(&mut rt);
    press(&mut rt, "material/source/apply");
    let graph = |rt: &Runtime<Studio>| {
        let workspace = rt.state().editor.host.store().workspace();
        ara_graph::Graph::from_document(workspace.doc(&doc).unwrap()).unwrap()
    };
    assert!(
        graph(&rt).edges.len() > 1,
        "the text's wires are in the graph"
    );
    // Back to the graph: a picked wire gets a reroute point.
    press(&mut rt, "material/view/graph");
    rt.touch(|s| super::graph::sync(s, "material"));
    let wire = graph(&rt).edges.keys().next().unwrap().clone();
    rt.update(|s| s.graph.pick(wire.clone()));
    rt.update(|s| super::commands::run(s, "graph.reroute", &crate::catalog::Target::None));
    assert_eq!(graph(&rt).editor.reroutes.get(&wire).map(Vec::len), Some(1));
}

#[test]
fn c_wraps_the_selection_in_a_comment_that_is_titled_and_removed_alone() {
    let mut rt = open();
    rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/notes.graph", "domain": "material"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
    });
    rt.update(|s| super::commands::run(s, "window.material", &crate::catalog::Target::None));
    rt.touch(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    let nodes = super::graph::node_ids(rt.state());
    let layout = |rt: &Runtime<Studio>| {
        let workspace = rt.state().editor.host.store().workspace();
        let doc = workspace.find("samples/notes.graph").expect("material");
        ara_graph::Graph::from_document(workspace.doc(doc.as_str()).expect("doc"))
            .expect("graph")
            .editor
    };
    rt.update(|s| super::graph::select(s, &nodes[..2]));
    rt.focus("graph/view");
    rt.frame(FRAME);
    rt.key(0x43);
    settle(&mut rt);
    rt.update(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    let groups = layout(&rt).groups;
    assert_eq!(groups.len(), 1, "C made a comment");
    let group = groups.values().next().expect("comment");
    assert_eq!(group.nodes, nodes[..2].to_vec());
    assert!(
        rt.find("material/comment/title").is_some(),
        "its title is typed in Details"
    );
    rt.type_text("Lighting");
    rt.key(0x0D);
    settle(&mut rt);
    rt.touch(|s| super::graph::sync(s, "material"));
    assert_eq!(
        layout(&rt).groups.values().next().expect("comment").label,
        "Lighting"
    );
    rt.focus("graph/view");
    rt.frame(FRAME);
    rt.key(0x2E);
    settle(&mut rt);
    rt.touch(|s| super::graph::sync(s, "material"));
    assert!(layout(&rt).groups.is_empty(), "Delete removed the comment");
    assert_eq!(
        super::graph::node_ids(rt.state()),
        nodes,
        "and only the comment"
    );
}

#[test]
fn command_d_duplicates_a_node_with_its_own_inline_defaults() {
    let mut rt = open();
    rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/twin.graph", "domain": "material"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc.clone()}));
        s.editor
            .apply_to(
                &doc,
                vec![json!({"cmd": "editor.material.node.add", "args": {"kind": "material.add", "position": [40, 40]}})],
                "Add",
            )
            .unwrap();
    });
    rt.update(|s| super::commands::run(s, "window.material", &crate::catalog::Target::None));
    rt.update(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    let before = super::graph::node_ids(rt.state());
    let defaults = super::graph::inline_values(rt.state());
    let add = defaults[0].0.clone();
    // The Add node is the one its defaults feed: pick it by its inline values' consumer.
    let workspace = rt.state().editor.host.store().workspace();
    let doc = workspace.find("samples/twin.graph").expect("material");
    let graph =
        ara_graph::Graph::from_document(workspace.doc(doc.as_str()).expect("doc")).expect("graph");
    let node = graph
        .edges
        .values()
        .find(|e| e.from.node == add)
        .expect("fed")
        .to
        .node
        .clone();
    rt.update(|s| super::graph::select(s, std::slice::from_ref(&node)));
    rt.focus("graph/view");
    rt.frame(FRAME);
    let command = if cfg!(target_os = "macos") {
        stillkit::el::Modifiers {
            meta: true,
            ..Default::default()
        }
    } else {
        stillkit::el::Modifiers {
            ctrl: true,
            ..Default::default()
        }
    };
    rt.key_with(0x44, command);
    settle(&mut rt);
    rt.update(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    let after = super::graph::node_ids(rt.state());
    assert_eq!(
        after.len(),
        before.len() + 1,
        "one copy, its defaults drawn inline"
    );
    let copied = super::graph::inline_values(rt.state());
    assert_eq!(
        copied.len(),
        defaults.len() * 2,
        "the copy has its own defaults"
    );
    let ids: std::collections::BTreeSet<_> = copied.iter().map(|(id, _)| id).collect();
    assert_eq!(ids.len(), copied.len(), "no default is shared");
}

#[test]
fn parameters_are_added_by_type_and_renamed_in_details() {
    let mut rt = open();
    rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/blackboard.graph", "domain": "material"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
    });
    rt.update(|s| super::commands::run(s, "window.material", &crate::catalog::Target::None));
    rt.update(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    let material = |rt: &Runtime<Studio>| {
        let workspace = rt.state().editor.host.store().workspace();
        let doc = workspace
            .find("samples/blackboard.graph")
            .expect("material");
        let graph = ara_graph::Graph::from_document(workspace.doc(doc.as_str()).expect("doc"))
            .expect("graph");
        ara_material::Material::from_graph(&graph).expect("material")
    };
    let before = material(&rt).parameters.len();
    press(&mut rt, "material/param/add");
    press(&mut rt, "material/param/add/vec3");
    let added = material(&rt);
    assert_eq!(added.parameters.len(), before + 1);
    let (id, parameter) = added
        .parameters
        .iter()
        .find(|(_, p)| p.ty == ara_material::Type::Vec3)
        .expect("a vec3 parameter");
    let id = id.clone();
    assert!(!parameter.name.is_empty());
    press(&mut rt, &format!("material/name/{id}"));
    rt.type_text("tint");
    rt.key(0x0D);
    settle(&mut rt);
    assert_eq!(
        material(&rt).parameters[&id].name,
        "tint",
        "renamed in place"
    );
}

/// The open effect at `path`, compiled.
fn effect_at(rt: &Runtime<Studio>, path: &str) -> ara_vfx::Effect {
    let workspace = rt.state().editor.host.store().workspace();
    let doc = workspace.find(path).expect("the effect");
    let graph =
        ara_graph::Graph::from_document(workspace.doc(doc.as_str()).expect("doc")).expect("graph");
    ara_vfx::Effect::from_graph(&graph).expect("it compiles")
}

#[test]
fn a_picked_vfx_module_is_edited_in_the_selection_panel() {
    let mut rt = open();
    rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/fire.graph", "domain": "vfx"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
    });
    rt.update(|s| super::commands::run(s, "window.vfx", &crate::catalog::Target::None));
    settle(&mut rt);
    assert!(
        rt.find("vfx/selection").is_some(),
        "the Selection panel shows"
    );
    // The first module with a number to type.
    let effect = effect_at(&rt, "samples/fire.graph");
    let emitter = &effect.emitters[0];
    let (stage, module, key) = [
        ("spawn", &emitter.stacks.spawn),
        ("initialize", &emitter.stacks.initialize),
        ("update", &emitter.stacks.update),
    ]
    .into_iter()
    .flat_map(|(stage, modules)| modules.iter().map(move |m| (stage, m)))
    .find_map(|(stage, m)| {
        let value = ara_json::to_value(&m.module).ok()?;
        let key = value
            .as_object()?
            .iter()
            .find(|(k, v)| *k != "op" && v.is_number())?
            .0
            .clone();
        Some((stage, m.name.clone(), key))
    })
    .expect("a module with a number");
    let item = format!("{}|{stage}|{module}", emitter.name);
    let row = rt
        .find(&format!("vfx/module/{item}"))
        .expect("the module's row");
    rt.pointer_move(row.center().x, row.center().y);
    rt.frame(FRAME);
    rt.pointer_down(row.center().x, row.center().y);
    rt.pointer_up(row.center().x, row.center().y);
    settle(&mut rt);
    let field = rt
        .find(&format!("vfx/set/{key}"))
        .expect("the picked module's value shows in the Selection");
    let selection = rt.find("vfx/selection").expect("selection");
    assert!(field.left >= selection.left, "in the Selection panel");
    rt.pointer_move(field.center().x, field.center().y);
    rt.frame(FRAME);
    rt.pointer_down(field.center().x, field.center().y);
    rt.pointer_up(field.center().x, field.center().y);
    rt.frame(FRAME);
    rt.type_text("7");
    rt.key(0x0D);
    settle(&mut rt);
    let effect = effect_at(&rt, "samples/fire.graph");
    let edited = [
        &effect.emitters[0].stacks.spawn,
        &effect.emitters[0].stacks.initialize,
        &effect.emitters[0].stacks.update,
    ]
    .into_iter()
    .flatten()
    .find(|m| m.name == module)
    .expect("the module");
    assert_eq!(
        ara_json::to_value(&edited.module).unwrap()[&key],
        json!(7.0)
    );
}

#[test]
fn the_size_over_life_curve_takes_a_key_and_a_dragged_one_in_one_step() {
    let mut rt = open();
    rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/fire.graph", "domain": "vfx"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
    });
    rt.update(|s| super::commands::run(s, "window.vfx", &crate::catalog::Target::None));
    settle(&mut rt);
    let size_curve = |rt: &Runtime<Studio>| {
        let effect = effect_at(rt, "samples/fire.graph");
        effect.emitters.iter().find_map(|e| {
            e.stacks.update.iter().find_map(|m| match &m.module {
                ara_vfx::Module::SizeOverLife { curve } => {
                    Some((e.name.clone(), m.name.clone(), curve.clone()))
                }
                _ => None,
            })
        })
    };
    let (emitter, module, curve) = size_curve(&rt).expect("a Size over Life module");
    press(&mut rt, &format!("vfx/module/{emitter}|update|{module}"));
    let plot = rt
        .find(&format!("curve/vfx/{emitter}/update/{module}"))
        .expect("the curve shows in the Selection");
    let (w, h) = (plot.width(), plot.height());
    let frame = super::curve::Frame::of(&curve, Some((0.0, 1.0)));
    let at = |(x, y): (f32, f32)| (plot.left + x, plot.top + y);
    // A double-click on the plot adds a key there.
    let (x, y) = at(frame.place((0.5, curve.evaluate(0.5) * 0.5), (w, h)));
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    let pointer = stillkit::Pointer {
        at: stillkit::Point::new(x, y),
        screen: (0, 0),
    };
    rt.handle(&stillkit::Event::DoubleClick(
        pointer,
        stillkit::Button::Left,
    ));
    rt.pointer_up(x, y);
    settle(&mut rt);
    let (_, _, added) = size_curve(&rt).expect("still there");
    assert_eq!(added.keys.len(), curve.keys.len() + 1, "a key added");
    let undo_depth = rt.state().editor.host.store().history().len();
    // The first key dragged up: higher, saved once on release.
    let first = added.keys[0];
    let frame = super::curve::Frame::of(&added, Some((0.0, 1.0)));
    let (x, y) = at(frame.place((first.time, first.value), (w, h)));
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    rt.pointer_down(x, y);
    for step in 1..=4 {
        rt.pointer_move(x, y - 6.0 * step as f32);
        rt.frame(FRAME);
    }
    assert_eq!(
        rt.state().editor.host.store().history().len(),
        undo_depth,
        "nothing saved mid-drag"
    );
    rt.pointer_up(x, y - 24.0);
    settle(&mut rt);
    let (_, _, dragged) = size_curve(&rt).expect("still there");
    assert!(
        dragged.keys[0].value > first.value,
        "{} > {}",
        dragged.keys[0].value,
        first.value
    );
    assert_eq!(dragged.keys[0].time, 0.0, "kept at the start of life");
    assert_eq!(
        rt.state().editor.host.store().history().len(),
        undo_depth + 1,
        "one undo step"
    );
}

#[test]
fn a_transition_s_blend_curve_shapes_it_and_straightens_back() {
    let mut rt = open();
    let doc = rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/run.animation", "domain": "animation"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
        for kind in ["state", "state", "transition"] {
            crate::animation_edit::edit(&mut s.editor, &doc, kind).unwrap();
        }
        doc
    });
    rt.update(|s| super::commands::run(s, "window.animation", &crate::catalog::Target::None));
    rt.update(|s| super::graph::sync(s, "animation"));
    settle(&mut rt);
    let blend = |rt: &Runtime<Studio>| {
        let workspace = rt.state().editor.host.store().workspace();
        crate::authoring::animation::controller(workspace.doc(&doc).expect("doc"))
            .expect("controller")
            .1
            .transitions[0]
            .blend
            .clone()
    };
    let transition = {
        let workspace = rt.state().editor.host.store().workspace();
        crate::authoring::animation::controller(workspace.doc(&doc).expect("doc"))
            .expect("controller")
            .1
            .transitions[0]
            .id
            .clone()
    };
    rt.update(|s| {
        s.graph
            .pick(crate::authoring::animation::transition_edge(&transition))
    });
    settle(&mut rt);
    assert_eq!(blend(&rt), None, "straight to start");
    let plot = rt
        .find(&format!("curve/animation/{transition}"))
        .expect("the blend curve shows with the transition");
    let (w, h) = (plot.width(), plot.height());
    let straight = ara_curve::Preset::Linear.curve();
    let frame = super::curve::Frame::of(&straight, Some((0.0, 1.0)));
    let (kx, ky) = frame.place((0.0, 0.0), (w, h));
    let (x, y) = (plot.left + kx, plot.top + ky);
    // Pick the first key, then pull its leaving handle flat: the fade eases in.
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert_eq!(blend(&rt), None, "picking a key changes nothing");
    // The handle lies along the straight line, 36 px out.
    let (ex, ey) = frame.place((1.0, 1.0), (w, h));
    let len = ((ex - kx).powi(2) + (ey - ky).powi(2)).sqrt();
    let handle = (x + (ex - kx) / len * 36.0, y + (ey - ky) / len * 36.0);
    rt.pointer_move(handle.0, handle.1);
    rt.frame(FRAME);
    rt.pointer_down(handle.0, handle.1);
    rt.pointer_move(handle.0 + 10.0, y);
    rt.frame(FRAME);
    rt.pointer_up(handle.0 + 10.0, y);
    rt.update(|s| super::graph::sync(s, "animation"));
    settle(&mut rt);
    let shaped = blend(&rt).expect("shaped");
    assert!(
        shaped.evaluate(0.25) < 0.2,
        "eases in: {}",
        shaped.evaluate(0.25)
    );
    press(&mut rt, "animation/transition/blend/reset");
    rt.update(|s| super::graph::sync(s, "animation"));
    assert_eq!(blend(&rt), None, "straight again");
}

#[test]
fn tab_stops_once_per_menu_bar_tree_and_toolbar_and_field_to_field_in_the_inspector() {
    let mut rt = open();
    settle(&mut rt);
    let mut stops: Vec<(String, String)> = Vec::new();
    for _ in 0..60 {
        rt.key(0x09);
        rt.frame(FRAME);
        if let Some(n) = rt.tree().into_iter().find(|n| n.focused) {
            let key = n.key.map(|k| k.to_string()).unwrap_or_default();
            stops.push((key, n.label.map(|l| l.to_string()).unwrap_or_default()));
        }
    }
    // Within one pass round the window (60 Tabs cover more than one here).
    let pass = stops
        .iter()
        .skip(1)
        .position(|s| *s == stops[0])
        .map_or(stops.len(), |at| at + 1);
    let stops = &stops[..pass];
    let count = |prefix: &str| stops.iter().filter(|(k, _)| k.starts_with(prefix)).count();
    assert_eq!(count("menu/"), 1, "the menu bar is one stop: {stops:?}");
    assert_eq!(
        count("row/"),
        1,
        "the Hierarchy is one stop, on its selected row"
    );
    assert_eq!(
        count("scene/view.tool."),
        1,
        "the Tools overlay is one stop"
    );
    assert!(
        !stops.iter().any(|(k, _)| k.ends_with("/scrub")),
        "scrub labels are drag handles, not stops"
    );
    let position: Vec<&str> = stops
        .iter()
        .filter(|(k, _)| k.starts_with("field/Transform.position/"))
        .map(|(_, l)| l.as_str())
        .collect();
    assert_eq!(
        position,
        ["Position X", "Position Y", "Position Z"],
        "field to field, named"
    );
}

#[test]
fn the_material_page_says_in_node_previews_are_not_available() {
    let mut rt = open();
    rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/paint.graph", "domain": "material"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.editor
            .apply_to(
                &doc,
                vec![json!({"cmd": "material.text.apply", "args": {"text": SHOWCASE}})],
                "Showcase",
            )
            .unwrap();
        s.act("editor.workspace", json!({"resource": doc}));
    });
    rt.update(|s| super::commands::run(s, "window.material", &crate::catalog::Target::None));
    settle(&mut rt);
    rt.update(|s| super::graph::sync(s, "material"));
    settle(&mut rt);
    assert!(
        rt.tree()
            .iter()
            .any(|n| n.text.as_deref() == Some("In-node previews are not available yet.")),
        "the Details say what is missing"
    );
}

#[test]
fn a_prefab_dragged_onto_a_hierarchy_row_lands_under_that_entity() {
    let mut rt = open();
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
    scan(&mut rt);
    rt.update(|s| {
        s.select(vec!["e_box".into()], false);
        super::commands::run(
            s,
            "create.prefab",
            &crate::catalog::Target::Entity("e_box".into()),
        );
    });
    // The new prefab shows in Assets once the browser rescans.
    let mut prefab = None;
    for _ in 0..400 {
        rt.touch(|s| {
            s.editor.tick();
            if let Some(b) = s.assets.as_mut() {
                b.refresh();
                b.poll();
            }
        });
        prefab = rt.touch(|s| {
            s.assets.as_ref().and_then(|b| {
                b.items
                    .iter()
                    .find(|a| a.kind == "prefab")
                    .map(|a| a.path.clone())
            })
        });
        if prefab.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let prefab = prefab.expect("the prefab shows in Assets");
    let folder = prefab.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
    rt.update(|s| s.act("editor.workspace", json!({"folder": folder})));
    settle(&mut rt);
    let tile = rt.find(&format!("asset/{prefab}")).expect("its tile");
    let row = rt.find("row/e_lamp").expect("the Lamp's row");
    rt.pointer_move(tile.center().x, tile.center().y);
    rt.frame(FRAME);
    rt.pointer_down(tile.center().x, tile.center().y);
    rt.pointer_move(tile.center().x + 20.0, tile.center().y - 20.0);
    rt.frame(FRAME);
    assert!(rt.state().local.asset_drag.is_some(), "dragging the prefab");
    // The host reports where things can drop while an asset is dragged.
    let keys = super::panels::drop_keys(rt.state());
    track(
        &mut rt,
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    rt.pointer_move(row.center().x, row.center().y);
    rt.frame(FRAME);
    rt.pointer_up(row.center().x, row.center().y);
    settle(&mut rt);
    let placed = rt
        .state()
        .resolved
        .iter()
        .filter(|(id, _)| !["e_box", "e_lamp", "e_sun", "e_post"].contains(&id.as_str()))
        .find(|(_, e)| e.parent.as_deref() == Some("e_lamp"));
    assert!(placed.is_some(), "an instance under the Lamp");
    // Onto an Inspector field that doesn't take a prefab (the Box's Model): it says so and
    // changes nothing.
    rt.update(|s| s.select(vec!["e_box".into()], false));
    settle(&mut rt);
    let field = rt
        .find("field/MeshRenderer.model")
        .expect("the Model field");
    let before = rt.state().resolved["e_box"].record.clone();
    rt.pointer_move(tile.center().x, tile.center().y);
    rt.frame(FRAME);
    rt.pointer_down(tile.center().x, tile.center().y);
    rt.pointer_move(tile.center().x + 20.0, tile.center().y - 20.0);
    rt.frame(FRAME);
    let keys = super::panels::drop_keys(rt.state());
    track(
        &mut rt,
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    rt.pointer_move(field.center().x, field.center().y);
    rt.frame(FRAME);
    rt.pointer_up(field.center().x, field.center().y);
    settle(&mut rt);
    assert_eq!(
        rt.state().resolved["e_box"].record,
        before,
        "nothing assigned"
    );
    let said = rt
        .state()
        .local
        .note
        .as_ref()
        .map(|(n, _)| n.clone())
        .unwrap_or_default();
    assert!(said.contains("doesn't take"), "it says why: {said}");
}

#[test]
fn a_script_fault_shows_its_stack_with_each_frame_s_source() {
    let mut rt = open();
    // Frames link only to files that exist in the project.
    let scripts = rt.state().editor.host.root().join("content/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    for name in ["PlayerControl.cs", "Targeting.cs"] {
        std::fs::write(scripts.join(name), "class C {}").unwrap();
    }
    let frame = |method: &str, file: Option<&str>, line: Option<u32>| json!({"method": method, "file": file, "line": line, "column": line.map(|_| 9)});
    let exception = json!({
        "schema": 1, "type": "System.InvalidOperationException", "message": "No target",
        "frames": [
            frame("PlayerControl.Update()", Some("content/scripts/PlayerControl.cs"), Some(42)),
            frame("Ara.Runtime.Tick()", None, None),
        ],
        "inners": [{
            "schema": 1, "type": "System.NullReferenceException", "message": "target was null",
            "frames": [frame("Targeting.Find()", Some("content/scripts/Targeting.cs"), Some(7))],
            "inners": [], "truncated": false
        }],
        "truncated": false
    });
    let diagnostic = ara_diag::Diagnostic::error(
        ara_diag::codes::UNRESOLVED_REF,
        "PlayerControl.Update threw",
    )
    .data(json!({"exception": exception}));
    let entry = json!({"seq": 9, "at": 0, "origin": "script", "diagnostic": serde_json::to_value(&diagnostic).unwrap()});
    let row = super::state::LogRow::from_entry(&entry).expect("a bus record reads");
    rt.update(|s| {
        s.logs = vec![row];
        s.act("editor.layout.show", json!({"panel": "console"}));
        s.local.console_open = Some(9);
    });
    settle(&mut rt);
    let texts: Vec<String> = rt
        .tree()
        .into_iter()
        .filter_map(|n| n.text.map(|t| t.to_string()))
        .collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("InvalidOperationException: No target")),
        "the fault"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Caused by System.NullReferenceException")),
        "its cause"
    );
    assert!(
        texts.iter().any(|t| t == "at Ara.Runtime.Tick()"),
        "a frame without a source"
    );
    assert!(
        rt.find("console/detail/frame/0").is_some(),
        "PlayerControl.cs:42 opens"
    );
    assert!(
        rt.find("console/detail/frame/1").is_none(),
        "no link without a file"
    );
    assert!(
        rt.find("console/detail/frame/2").is_some(),
        "the cause's frame opens too"
    );
}

#[test]
fn the_game_view_plays_at_a_fixed_resolution_scaled_to_fit() {
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "game.aspect.1920x1080", &crate::catalog::Target::None));
    let aspect = rt.state().editor.state.settings["game.aspect"]
        .as_str()
        .map(str::to_owned);
    assert_eq!(aspect.as_deref(), Some("1920x1080"), "kept as the setting");
    assert_eq!(super::game::resolution("1920x1080"), Some([1920, 1080]));
    assert_eq!(
        super::game::resolution("16x9"),
        None,
        "an aspect, not a size"
    );
    assert_eq!(super::game::resolution("free"), None);
}

#[test]
fn emitters_add_duplicate_and_delete_but_one_stays() {
    let mut rt = open();
    let doc = rt.touch(|s| {
        let doc = s
            .editor
            .action(
                "editor.resource.create",
                json!({"path": "samples/sparks.graph", "domain": "vfx"}),
            )
            .unwrap()["doc"]
            .as_str()
            .unwrap()
            .to_owned();
        s.act("editor.workspace", json!({"resource": doc}));
        doc
    });
    rt.update(|s| super::commands::run(s, "window.vfx", &crate::catalog::Target::None));
    settle(&mut rt);
    let names = |rt: &Runtime<Studio>| -> Vec<String> {
        effect_at(rt, "samples/sparks.graph")
            .emitters
            .into_iter()
            .map(|e| e.name)
            .collect()
    };
    let start = names(&rt).len();
    press(&mut rt, "vfx/emitter/add");
    assert_eq!(names(&rt).len(), start + 1, "added");
    let first = names(&rt)[0].clone();
    rt.update(|s| crate::vfx_edit::emitter_edit(&mut s.editor, &doc, &first, "duplicate").unwrap());
    assert_eq!(names(&rt).len(), start + 2, "duplicated");
    for name in names(&rt).into_iter().skip(1) {
        rt.update(|s| crate::vfx_edit::emitter_edit(&mut s.editor, &doc, &name, "delete").unwrap());
    }
    assert_eq!(names(&rt), std::slice::from_ref(&first), "down to one");
    let last = rt.touch(|s| crate::vfx_edit::emitter_edit(&mut s.editor, &doc, &first, "delete"));
    assert!(last.is_err(), "the last one stays");
}

#[test]
fn assets_hide_their_folder_tree_for_one_column() {
    let mut rt = open();
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
    scan(&mut rt);
    settle(&mut rt);
    assert!(rt.find("assets/tree/").is_some(), "the tree shows");
    press(&mut rt, "assets/menu");
    press(&mut rt, "assets/menu/tree");
    assert!(rt.find("assets/tree/").is_none(), "one column");
    press(&mut rt, "assets/menu");
    press(&mut rt, "assets/menu/tree");
    assert!(rt.find("assets/tree/").is_some(), "back again");
}

#[test]
fn the_hierarchy_sorts_by_name_on_request() {
    let mut rt = open();
    settle(&mut rt);
    let order = |rt: &Runtime<Studio>| {
        let mut rows: Vec<(f32, &str)> = ["e_box", "e_lamp", "e_sun", "e_post"]
            .into_iter()
            .map(|id| (rt.find(&format!("row/{id}")).unwrap().top, id))
            .collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        rows.into_iter().map(|(_, id)| id).collect::<Vec<_>>()
    };
    assert_eq!(
        order(&rt),
        ["e_box", "e_lamp", "e_sun", "e_post"],
        "authored order"
    );
    press(&mut rt, "hierarchy/filters");
    press(&mut rt, "hierarchy/filters/alphabetical");
    assert_eq!(
        order(&rt),
        ["e_box", "e_lamp", "e_post", "e_sun"],
        "by name"
    );
}

#[test]
fn a_console_row_opens_its_details() {
    let mut rt = open();
    let diagnostic = ara_diag::Diagnostic::error(
        ara_diag::codes::UNRESOLVED_REF,
        "Missing @textures/rock.png",
    );
    let entry = json!({"seq": 7, "at": 0, "origin": "engine", "diagnostic": serde_json::to_value(&diagnostic).unwrap()});
    let row = super::state::LogRow::from_entry(&entry).expect("a bus record reads");
    rt.update(|s| {
        s.logs = vec![row];
        s.act("editor.layout.show", json!({"panel": "console"}));
    });
    settle(&mut rt);
    let at = rt.find("console/row/7").expect("the row shows");
    rt.pointer_down(at.center().x, at.center().y);
    rt.pointer_up(at.center().x, at.center().y);
    settle(&mut rt);
    assert_eq!(rt.state().local.console_open, Some(7));
}

/// What one click costs at Retina scale: event, view, layout and a full CPU repaint.
/// `cargo test -p ara-editor --release --lib studio::tests::click_cost -- --ignored --nocapture`
#[test]
#[ignore = "timing probe"]
fn click_cost() {
    let mut rt = open();
    rt.set_scale(2.0);
    rt.resize((2984, 1884));
    for _ in 0..5 {
        rt.frame(FRAME);
    }
    let _ = rt.snap();
    for id in ["e_lamp", "e_box", "e_lamp", "e_box"] {
        let at = rt.find(&format!("row/{id}")).unwrap();
        let t = std::time::Instant::now();
        rt.pointer_down(at.center().x, at.center().y);
        rt.pointer_up(at.center().x, at.center().y);
        let handled = t.elapsed();
        rt.frame(FRAME);
        let framed = t.elapsed();
        let _ = rt.snap();
        eprintln!(
            "CLICK {id}: handlers {handled:?}, frame {:?}, paint {:?}",
            framed - handled,
            t.elapsed() - framed
        );
    }
    // What opening a menu repaints: its own area, not the window.
    let area = |rt: &Runtime<Studio>| rt.damage().map_or(0.0, |d| d.width() * d.height());
    for (name, key) in [
        ("dropdown", "field/MeshRenderer.shape"),
        ("add component", "inspector/add"),
    ] {
        rt.frame(FRAME);
        let at = rt.find(key).unwrap();
        rt.pointer_down(at.center().x, at.center().y);
        rt.pointer_up(at.center().x, at.center().y);
        let (mut total, mut regions) = (0.0, 0.0_f32);
        for _ in 0..20 {
            let before = rt.list().clone();
            rt.frame(FRAME);
            total = f32::max(total, area(&rt));
            let parts: Vec<_> = rt.list().damage_regions(&before);
            regions = regions.max(parts.iter().map(|d| d.width() * d.height()).sum());
        }
        let window = 2984.0 * 1884.0;
        eprintln!(
            "DAMAGE {name}: union {:.0}%, regions {:.0}% of the window",
            total / window * 100.0,
            regions / window * 100.0
        );
        rt.key(0x1B);
        for _ in 0..20 {
            rt.frame(FRAME);
        }
    }
    // A dropdown: the press, then each frame of its opening until it settles.
    let at = rt.find("field/MeshRenderer.shape").unwrap();
    let t = std::time::Instant::now();
    rt.pointer_down(at.center().x, at.center().y);
    rt.pointer_up(at.center().x, at.center().y);
    eprintln!("DROPDOWN press {:?}", t.elapsed());
    for i in 0..30 {
        let f = std::time::Instant::now();
        let more = rt.frame(FRAME);
        let framed = f.elapsed();
        let _ = rt.snap();
        eprintln!(
            "DROPDOWN frame {i}: frame {framed:?}, paint {:?}",
            f.elapsed() - framed
        );
        if !more && !rt.animating() {
            break;
        }
    }
}

/// The Linux studio path end to end: a GPU stillkit window, a native layer (a Wayland
/// subsurface), and a studio layer presenting into it at three sizes.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "needs a Wayland session and a Vulkan GPU (the Linux gate)"]
fn a_wayland_layer_presents_and_resizes() -> anyhow::Result<()> {
    use crate::world::EditWorldView as _;
    use anyhow::Context;
    let mut rt = open();
    stillkit::platform::set_render(stillkit::platform::Render::Gpu);
    let surface = stillkit::Surface::new(
        stillkit::Kind::App { min: (320, 240) },
        "native layer test",
        stillkit::Role::Window,
    )?;
    let target = super::present::target(surface.native_layer("test")?)?;
    assert!(
        matches!(target, super::present::Target::Wayland { .. }),
        "{target:?}"
    );
    let scene = rt.touch(|s| -> anyhow::Result<_> {
        let view = s.editor.view()?;
        view.scene(
            s.editor.host.root(),
            &s.editor.state,
            None,
            &mut s.editor.models,
        )
    })?;
    let cache = rt.state().editor.host.root().join(".ara/gpu");
    let (_device, renderer) = crate::studio::viewport::prepare_renderer(
        scene,
        [64, 64],
        super::present_extensions(),
        cache,
        false,
        Default::default(),
        |_| Ok(true),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?
    .context("the renderer started")?;
    // SAFETY: the renderer outlives the layer (retired below); this thread submits.
    let mut layer = unsafe { super::layer::Layer::new(&renderer, [])? };
    let list = ara_ui::paint::DisplayList {
        commands: Vec::new(),
        glyphs: Vec::new(),
    };
    let result = (|| -> anyhow::Result<()> {
        for extent in [[200, 120], [260, 160], [128, 96]] {
            let rect = stillkit::Rect::new(10.0, 10.0, extent[0] as f32, extent[1] as f32);
            surface.place_native_layer("test", Some(rect));
            for frame in 0..3 {
                let started = std::time::Instant::now();
                while !layer.ready(&renderer)? {
                    anyhow::ensure!(
                        started.elapsed() < Duration::from_secs(5),
                        "frame {frame} at {extent:?} never left the GPU"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                // SAFETY: the layer's subsurface is live until released below.
                unsafe { layer.draw(&renderer, target, extent, 1.0, &list, &[])? };
            }
        }
        surface.place_native_layer("test", None);
        Ok(())
    })();
    layer.retire(&renderer);
    surface.release_native_layer("test");
    result
}

#[test]
fn the_rect_tool_resizes_from_an_edge_keeping_the_opposite_one_in_one_undo_step() {
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "view.tool.rect", &crate::catalog::Target::None));
    assert_eq!(rt.state().editor.state.tool, crate::state::Tool::Rect);
    // The host publishes the presented bounds; here, the unit box's.
    rt.update(|s| {
        s.local.rect_bounds = Some(super::rect_tool::RectBounds {
            rotation: glam::Quat::IDENTITY,
            min: glam::Vec3::splat(-0.5),
            max: glam::Vec3::splat(0.5),
        })
    });
    settle(&mut rt);
    let view = rt.find("scene/view").unwrap();
    let size = (view.width(), view.height());
    let c = super::rect_tool::corners(rt.state(), size).expect("the rect shows");
    // The edge from corner 1 to 2 is the high side of the first axis; drag it outward.
    let mid = (c[1] + c[2]) * 0.5;
    let out = (mid - (c[0] + c[2]) * 0.5).normalize();
    let at = |d: f32| (view.left + mid.x + out.x * d, view.top + mid.y + out.y * d);
    let before = rt.state().editor.host.store().revision();
    rt.pointer_move(at(0.0).0, at(0.0).1);
    rt.frame(FRAME);
    assert!(rt.state().local.rect_hover.is_some(), "the edge lights");
    rt.pointer_down(at(0.0).0, at(0.0).1);
    for step in 1..=6 {
        rt.pointer_move(at(step as f32 * 10.0).0, at(step as f32 * 10.0).1);
        rt.frame(FRAME);
    }
    assert!(
        rt.state().local.rect_drag.is_some(),
        "the drag holds the edge"
    );
    rt.pointer_up(at(60.0).0, at(60.0).1);
    rt.frame(FRAME);
    assert_eq!(
        rt.state().editor.host.store().revision(),
        before + 1,
        "one transaction"
    );
    let record = &rt.state().resolved["e_box"].record["Transform"];
    let scale: Vec<f64> = (0..3)
        .map(|i| record["scale"][i].as_f64().unwrap())
        .collect();
    let position: Vec<f64> = (0..3)
        .map(|i| record["position"][i].as_f64().unwrap())
        .collect();
    let axis = (0..3)
        .find(|&i| (scale[i] - 1.0).abs() > 0.05)
        .expect("one axis grew");
    assert!(scale[axis] > 1.0, "{scale:?}");
    assert_eq!(
        (0..3).filter(|&i| (scale[i] - 1.0).abs() > 1e-4).count(),
        1,
        "{scale:?}"
    );
    // The opposite edge stays: the centre moves by half the growth, toward the dragged side.
    assert!(
        (position[axis].abs() - (scale[axis] - 1.0) * 0.5).abs() < 1e-3,
        "{position:?} {scale:?}"
    );
}

#[test]
fn the_game_scale_slider_zooms_live_and_keeps_the_scale_on_release() {
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "window.game", &crate::catalog::Target::None));
    settle(&mut rt);
    assert_eq!(super::game::zoom(rt.state()), 1.0);
    let track = rt.find("game/scale").expect("the Scale slider shows");
    let (y, from) = (track.center().y, track.left + 1.0);
    rt.pointer_move(from, y);
    rt.frame(FRAME);
    rt.pointer_down(from, y);
    let to = track.left + track.width() * 0.75;
    rt.pointer_move(to, y);
    rt.frame(FRAME);
    let live = super::game::zoom(rt.state());
    assert!((live - 4.0).abs() < 0.15, "live while dragging: {live}");
    assert_eq!(
        rt.state().editor.state.settings["game.scale"].as_f64(),
        Some(1.0),
        "written only on the release"
    );
    rt.pointer_up(to, y);
    settle(&mut rt);
    let kept = rt.state().editor.state.settings["game.scale"].as_f64();
    assert!(kept.is_some_and(|v| (v - 4.0).abs() < 0.15), "{kept:?}");
    assert_eq!(super::game::zoom(rt.state()), kept.unwrap() as f32);
}

#[test]
fn align_with_view_puts_the_selection_at_the_camera_facing_its_way() {
    let mut rt = open();
    rt.update(|s| s.select(vec!["e_lamp".into()], false));
    rt.update(|s| super::commands::run(s, "transform.align_view", &crate::catalog::Target::None));
    settle(&mut rt);
    let camera = rt.state().editor.state.camera;
    let world = crate::world::world_transform(&rt.state().resolved, "e_lamp").unwrap();
    let (_, rotation, position) = world.to_scale_rotation_translation();
    assert!(
        position.abs_diff_eq(camera.position(), 1e-3),
        "{position} vs {}",
        camera.position()
    );
    let forward = (camera.target - camera.position()).normalize();
    assert!(
        (rotation * glam::Vec3::NEG_Z).abs_diff_eq(forward, 1e-3),
        "faces where the view looks"
    );
}

#[test]
fn dragging_a_component_header_reorders_the_sections_without_folding() {
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "window.inspector", &crate::catalog::Target::None));
    settle(&mut rt);
    let order = |rt: &Runtime<Studio>| -> Vec<String> {
        let mut shown = vec!["Transform".to_owned(), "MeshRenderer".to_owned()];
        super::inspector::sort_components(rt.state(), &mut shown);
        shown
    };
    assert_eq!(order(&rt), ["Transform", "MeshRenderer"]);
    let mesh = rt
        .find("component/MeshRenderer")
        .expect("the section shows");
    let top = rt.find("component/Transform").unwrap();
    rt.pointer_move(mesh.center().x, mesh.center().y);
    rt.frame(FRAME);
    rt.pointer_down(mesh.center().x, mesh.center().y);
    rt.frame(FRAME);
    // The host publishes the headers' rects while a component drags.
    track(&mut rt, &["component/Transform", "component/MeshRenderer"]);
    for step in 1..=6 {
        let y = mesh.center().y + (top.top - mesh.center().y) * step as f32 / 6.0;
        rt.pointer_move(mesh.center().x, y);
        rt.frame(FRAME);
    }
    assert_eq!(
        rt.state().local.component_drag,
        Some((
            "MeshRenderer".into(),
            Some(super::inspector::Slot::Before("Transform".into()))
        )),
        "the slot follows the pointer"
    );
    rt.pointer_up(mesh.center().x, top.top);
    settle(&mut rt);
    assert_eq!(order(&rt), ["MeshRenderer", "Transform"]);
    assert!(
        rt.state().local.folded.is_empty(),
        "the drop folded nothing"
    );
    // A plain click still folds.
    press(&mut rt, "component/Transform");
    assert!(rt.state().local.folded.contains("Transform"));
}

#[test]
fn save_as_copies_the_open_scene_and_opens_the_copy() {
    let mut rt = open();
    let root = rt.state().editor.host.root().to_owned();
    let content = root.join(ara_doc::AUTHORING_ROOT);
    let original = rt.state().editor.state.doc.clone();
    let outside = rt.touch(|s| super::asset_ops::save_as(s, &root.join("elsewhere")));
    assert!(
        outside.is_err_and(|e| e.to_string().contains("inside")),
        "a place outside the content folder is refused"
    );
    rt.touch(|s| super::asset_ops::save_as(s, &content.join("levels").join("copy")))
        .expect("queued");
    let file = content.join("levels/copy.scene");
    finish_file_jobs(&mut rt, &file);
    settle(&mut rt);
    let state = &rt.state().editor;
    assert_ne!(state.state.doc, original, "the copy is open");
    assert_eq!(
        state.host.store().workspace().path_of(&state.state.doc),
        Some(format!("{}/levels/copy.scene", ara_doc::AUTHORING_ROOT).as_str())
    );
    assert!(
        rt.state().resolved.contains_key("e_box"),
        "the copy keeps the scene's entities"
    );
}

#[test]
fn the_profiler_shows_memory_and_what_each_scope_allocated() {
    let mut rt = open();
    rt.update(|s| {
        s.profile = std::sync::Arc::new(sample_profile());
        super::commands::run(s, "window.profiler", &crate::catalog::Target::None);
    });
    settle(&mut rt);
    let texts: Vec<String> = rt
        .tree()
        .into_iter()
        .filter_map(|n| n.text.map(|t| t.to_string()))
        .collect();
    for wanted in [
        "CPU heap",
        "185.6 MB",
        "+64.0 KB",
        "GPU textures",
        "412.0 MB",
        "Alloc",
    ] {
        assert!(texts.iter().any(|t| t == wanted), "{wanted} in {texts:?}");
    }
    // A capture without a counting allocator says so instead of zeros.
    rt.update(|s| {
        let mut bare = sample_profile();
        for f in &mut bare.frames {
            f.memory = ara_profile::Memory::default();
        }
        s.profile = std::sync::Arc::new(bare);
    });
    settle(&mut rt);
    assert!(
        rt.tree()
            .into_iter()
            .filter_map(|n| n.text)
            .any(|t| t.starts_with("Not measured")),
        "the module says why it is empty"
    );
}

#[test]
fn stack_frames_outside_the_project_never_open() {
    let mut rt = open();
    let root = rt.state().editor.host.root().to_owned();
    std::fs::create_dir_all(root.join("content/scripts")).unwrap();
    std::fs::write(root.join("content/scripts/Mover.cs"), "class Mover {}").unwrap();
    // Outside the project, beside it: reachable only through `..`.
    std::fs::write(root.with_extension("bat"), "echo").unwrap();
    let outside = format!(
        "../{}",
        root.with_extension("bat")
            .file_name()
            .unwrap()
            .to_string_lossy()
    );
    let absolute = root
        .join("content/scripts/Mover.cs")
        .to_string_lossy()
        .into_owned();
    for file in [
        absolute.as_str(),
        "/etc/passwd",
        "C:\\Windows\\x.bat",
        "\\\\host\\share\\x.exe",
        outside.as_str(),
        "content/scripts/../../../x.bat",
        "content\\scripts\\..\\..\\x.bat",
        "content/scripts/Missing.cs",
    ] {
        assert_eq!(
            super::panels::frame_target(&root, file),
            None,
            "{file} is refused"
        );
    }
    assert!(
        super::panels::frame_target(&root, "content/scripts/Mover.cs").is_some(),
        "a project file opens"
    );
    // In the Console: the refused frames read as text, the project file is a link.
    let frame = |i: u32, file: &str| json!({"method": format!("M{i}()"), "file": file, "line": 3, "column": 1});
    let exception = json!({"schema": 1, "type": "System.Exception", "message": "boom", "frames": [
        frame(0, "C:\\Windows\\x.bat"),
        frame(1, "\\\\host\\share\\x.exe"),
        frame(2, &outside),
        frame(3, "content/scripts/Mover.cs"),
    ], "inners": [], "truncated": false});
    let diagnostic = ara_diag::Diagnostic::error(ara_diag::codes::UNRESOLVED_REF, "Mover threw")
        .data(json!({"exception": exception}));
    let entry = json!({"seq": 4, "at": 0, "origin": "script", "diagnostic": serde_json::to_value(&diagnostic).unwrap()});
    let row = super::state::LogRow::from_entry(&entry).expect("a bus record reads");
    rt.update(|s| {
        s.logs = vec![row];
        s.act("editor.layout.show", json!({"panel": "console"}));
        s.local.console_open = Some(4);
    });
    settle(&mut rt);
    for i in 0..3 {
        assert!(
            rt.find(&format!("console/detail/frame/{i}")).is_none(),
            "frame {i} is no link"
        );
        assert!(
            rt.find(&format!("console/detail/source/{i}")).is_some(),
            "frame {i} reads as text"
        );
    }
    assert!(
        rt.find("console/detail/frame/3").is_some(),
        "the project file links"
    );
    let _ = std::fs::remove_file(root.with_extension("bat"));
}

#[test]
fn agents_capture_the_studio_window_and_its_layout() {
    let mut rt = open();
    settle(&mut rt);
    let root = rt.state().editor.host.root().to_owned();
    for (target, file) in [("editor", "studio.png"), ("layout", "layout.json")] {
        let job = rt.touch(|s| {
            s.editor
                .action(
                    "capture",
                    json!({"target": target, "path": format!(".ara/captures/{file}")}),
                )
                .unwrap()["job"]
                .as_str()
                .unwrap()
                .to_owned()
        });
        super::capture::serve(&mut rt, None);
        let started = std::time::Instant::now();
        let status = loop {
            let status = rt.touch(|s| {
                s.editor.tick();
                s.editor.action("jobs.status", json!({"job": job})).unwrap()
            });
            if status["status"] != "running" {
                break status;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{target}: {status}"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status["status"], "done", "{target}: {status}");
        let report = match target {
            "editor" => {
                let png = std::fs::read(root.join(".ara/captures/studio.png")).unwrap();
                assert!(png.starts_with(b"\x89PNG"), "a PNG");
                std::fs::read_to_string(root.join(".ara/captures/studio.layout.json")).unwrap()
            }
            _ => std::fs::read_to_string(root.join(".ara/captures/layout.json")).unwrap(),
        };
        assert!(
            report.contains("scene/view"),
            "{target}: the layout names the Scene view"
        );
    }
}

/// Starts a capture job of `target` with no Scene on screen and serves it as the host loop
/// does; returns its final status.
fn capture_without_scene(rt: &mut Runtime<Studio>, target: &str) -> Value {
    let job = rt.touch(|s| {
        s.editor
            .action(
                "capture",
                json!({"target": target, "path": format!(".ara/captures/{target}-hidden.png")}),
            )
            .unwrap()["job"]
            .as_str()
            .unwrap()
            .to_owned()
    });
    super::capture::serve(rt, None);
    let started = std::time::Instant::now();
    loop {
        let status = rt.touch(|s| {
            s.editor.tick();
            s.editor.action("jobs.status", json!({"job": job})).unwrap()
        });
        if status["status"] != "running" {
            return status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{target}: {status}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_viewport_capture_with_the_scene_hidden_fails_saying_so() {
    let mut rt = open();
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "lighting"})));
    settle(&mut rt);
    let status = capture_without_scene(&mut rt, "viewport");
    assert_ne!(status["status"], "done", "{status}");
    assert!(
        status.to_string().contains("Scene view is not shown"),
        "the failure names the cause: {status}"
    );
    // The other targets do not need the Scene.
    for target in ["editor", "layout"] {
        let status = capture_without_scene(&mut rt, target);
        assert_eq!(status["status"], "done", "{target}: {status}");
    }
}

#[test]
fn captures_of_the_window_and_its_layout_complete_while_it_is_minimised() {
    let mut rt = open();
    settle(&mut rt);
    rt.set_visible(false);
    rt.frame(FRAME);
    for target in ["editor", "layout"] {
        let status = capture_without_scene(&mut rt, target);
        assert_eq!(status["status"], "done", "{target}: {status}");
    }
    let status = capture_without_scene(&mut rt, "viewport");
    assert!(
        status.to_string().contains("Scene view is not shown"),
        "{status}"
    );
}

/// The `editor` capture says the native views are not in its picture.
#[test]
fn the_editor_capture_says_the_scene_is_not_in_it() {
    let mut rt = open();
    settle(&mut rt);
    let status = capture_without_scene(&mut rt, "editor");
    assert!(
        status.to_string().contains("native GPU layers"),
        "the result points at `viewport`: {status}"
    );
}

/// Window > Lighting in front of the Scene at High: Recapture starts a bake, which only
/// advances through Scene renders. Polling alone leaves it running; the unpresented render
/// the loop runs for it completes it.
#[test]
fn recapturing_probes_says_it_is_not_available_and_starts_no_job() {
    use crate::catalog::Target;
    let mut rt = open();
    rt.update(|s| super::commands::run(s, "create.light.probe", &Target::None));
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "lighting"})));
    settle(&mut rt);
    assert!(
        rt.find("lighting/probe/unavailable").is_some(),
        "the page says so beside the button"
    );
    assert!(
        rt.find("lighting/probe/recapture").is_some(),
        "the button keeps its place"
    );
    // Disabled: pressing it, or running the command, starts nothing.
    press(&mut rt, "lighting/probe/recapture");
    rt.update(|s| super::commands::run(s, "lighting.probe.recapture", &Target::None));
    settle(&mut rt);
    assert!(!super::lighting::capturing(rt.state()), "no capture job");
}

#[test]
fn a_placeholder_probe_report_is_not_a_quality_problem() {
    let mut rt = open();
    let report = |version: u64| -> ara_runtime_render::probes::ProbeSnapshot {
        serde_json::from_value(json!({"version":version,"sceneRevision":0,"probes":[],"capabilities":{
            "reflectionProbes":false,"giDiffuse":false,"overdraw":false,
            "reasons":{"reflectionProbes":"The Scene renderer has not published a probe snapshot"}}}))
        .unwrap()
    };
    rt.update(|s| s.local.probe_status = Some(report(0)));
    assert_eq!(super::lighting::probes_unavailable(rt.state()), None);
    rt.update(|s| s.local.probe_status = Some(report(3)));
    assert_eq!(
        super::lighting::probes_unavailable(rt.state()),
        Some("The Scene renderer has not published a probe snapshot")
    );
}

#[test]
fn agents_capture_the_scene_with_entity_marks_on_this_gpu() {
    use crate::world::EditWorldView as _;
    let mut rt = open();
    let root = rt.state().editor.host.root().to_owned();
    let scene = rt.touch(|s| {
        let view = s.editor.view().unwrap();
        view.scene(
            s.editor.host.root(),
            &s.editor.state,
            None,
            &mut s.editor.models,
        )
        .unwrap()
    });
    let (_device, mut renderer) = crate::studio::viewport::prepare_renderer(
        scene,
        [320, 240],
        super::present_extensions(),
        root.join(".ara/gpu"),
        false,
        Default::default(),
        |_| Ok(true),
    )
    .unwrap()
    .expect("the renderer starts on this GPU");
    let mut view = crate::studio::viewport::SceneView::default();
    // The Scene draws through Renderlab, as the live view does, before an agent reads it.
    rt.touch(|s| {
        let document = s.editor.view().unwrap();
        view.render_capture(&mut renderer, &s.editor, &document, 1)
    })
    .unwrap();
    let job = rt.touch(|s| {
        s.editor
            .action(
                "capture",
                json!({"target": "viewport", "path": ".ara/captures/scene.png", "marks": 8}),
            )
            .unwrap()["job"]
            .as_str()
            .unwrap()
            .to_owned()
    });
    super::capture::serve(&mut rt, Some((&mut renderer, &view)));
    let started = std::time::Instant::now();
    let status = loop {
        let status = rt.touch(|s| {
            s.editor.tick();
            s.editor.action("jobs.status", json!({"job": job})).unwrap()
        });
        if status["status"] != "running" {
            break status;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "{status}");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status["status"], "done", "{status}");
    assert!(root.join(".ara/captures/scene.png").is_file());
    assert!(root.join(".ara/captures/scene.entities.json").is_file());
    view.destroy(&mut renderer).unwrap();
}

/// The `window` target is the editor window with the Scene in it: the picture is the size of
/// the window's own snapshot, the Scene view's middle is the render (not the window's hole),
/// and the chrome over it is the UI's.
#[test]
fn agents_capture_the_window_with_the_scene_in_it_on_this_gpu() {
    use crate::world::EditWorldView as _;
    let mut rt = open();
    settle(&mut rt);
    let root = rt.state().editor.host.root().to_owned();
    let scene = rt.touch(|s| {
        s.editor
            .view()
            .unwrap()
            .scene(
                s.editor.host.root(),
                &s.editor.state,
                None,
                &mut s.editor.models,
            )
            .unwrap()
    });
    let (_device, mut renderer) = crate::studio::viewport::prepare_renderer(
        scene,
        [320, 240],
        super::present_extensions(),
        root.join(".ara/gpu"),
        false,
        Default::default(),
        |_| Ok(true),
    )
    .unwrap()
    .expect("the renderer starts on this GPU");
    let mut view = crate::studio::viewport::SceneView::default();
    rt.touch(|s| {
        let document = s.editor.view().unwrap();
        view.render_capture(&mut renderer, &s.editor, &document, 1)
    })
    .unwrap();
    let job = rt.touch(|s| {
        s.editor
            .action(
                "capture",
                json!({"target": "window", "path": ".ara/captures/window.png"}),
            )
            .unwrap()["job"]
            .as_str()
            .unwrap()
            .to_owned()
    });
    super::capture::serve(&mut rt, Some((&mut renderer, &view)));
    let started = std::time::Instant::now();
    let status = loop {
        let status = rt.touch(|s| {
            s.editor.tick();
            s.editor.action("jobs.status", json!({"job": job})).unwrap()
        });
        if status["status"] != "running" {
            break status;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "{status}");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status["status"], "done", "{status}");
    let picture = image::open(root.join(".ara/captures/window.png"))
        .unwrap()
        .to_rgba8();
    let ui = rt.snap().unwrap();
    assert_eq!(
        picture.dimensions(),
        (ui.width, ui.height),
        "the window's size"
    );
    let rect = rt.find("scene/view").unwrap();
    let scale = rt.scale();
    let (x, y) = (
        ((rect.left + rect.width() / 2.0) * scale) as u32,
        ((rect.top + rect.height() / 2.0) * scale) as u32,
    );
    let [r, g, b, a] = picture.get_pixel(x, y).0;
    // Opaque (blending a faint UI pixel over the Scene may round down by one).
    assert!(a >= 250, "alpha {a}");
    assert!(
        u32::from(r) + u32::from(g) + u32::from(b) > 30,
        "the Scene's render is there, not the hole: {r} {g} {b}"
    );
    // The chrome is the UI's: the title bar is not the Scene's sky.
    assert_ne!(picture.get_pixel(x, 4).0, picture.get_pixel(x, y).0);
    view.destroy(&mut renderer).unwrap();
}

/// Drags the rect's edge from corner 1 to 2 (its first axis' high side) 60 px outward and
/// lets go; the rect as it showed just before the release.
fn drag_rect_edge(rt: &mut Runtime<Studio>) -> super::rect_tool::Frame {
    let view = rt.find("scene/view").unwrap();
    let c = super::rect_tool::corners(rt.state(), (view.width(), view.height()))
        .expect("the rect shows");
    let mid = (c[1] + c[2]) * 0.5;
    let out = (mid - (c[0] + c[2]) * 0.5).normalize();
    let at = |d: f32| (view.left + mid.x + out.x * d, view.top + mid.y + out.y * d);
    rt.pointer_move(at(0.0).0, at(0.0).1);
    rt.frame(FRAME);
    rt.pointer_down(at(0.0).0, at(0.0).1);
    for step in 1..=6 {
        rt.pointer_move(at(step as f32 * 10.0).0, at(step as f32 * 10.0).1);
        rt.frame(FRAME);
    }
    let shown = super::rect_tool::shown(rt.state()).expect("the drag shows its rect");
    rt.pointer_up(at(60.0).0, at(60.0).1);
    rt.frame(FRAME);
    shown
}

/// Creates box entities (`id`, Y rotation in degrees, position) and selects them with the
/// Rect tool, the bounds published as the host would (a unit box each, in the frame the
/// selection gets).
fn rect_boxes(rt: &mut Runtime<Studio>, boxes: &[(&str, f32, [f32; 3])]) {
    let create = boxes
        .iter()
        .map(|(id, turn, at)| json!({"cmd":"entity.create","args":{"id":id,"name":id,"components":{"Transform":{"position":at,"rotation":[0,turn,0]},"MeshRenderer":{"shape":"box"}}}}))
        .collect();
    rt.touch(|s| s.editor.apply(create, "Create", None).unwrap());
    rt.update(|s| s.resolve());
    let ids: Vec<String> = boxes.iter().map(|(id, ..)| id.to_string()).collect();
    rt.update(|s| {
        s.select(ids.clone(), false);
        super::commands::run(s, "view.tool.rect", &crate::catalog::Target::None);
        let rotation = super::rect_tool::frame_rotation(&s.resolved, &ids);
        let into = glam::Mat4::from_quat(rotation.inverse());
        let (mut min, mut max) = (glam::Vec3::MAX, glam::Vec3::MIN);
        for id in &ids {
            let world = crate::world::world_transform(&s.resolved, id).unwrap();
            for i in 0..8 {
                let local = glam::Vec3::new(
                    [-0.5, 0.5][i & 1],
                    [-0.5, 0.5][(i >> 1) & 1],
                    [-0.5, 0.5][(i >> 2) & 1],
                );
                let p = (into * world).transform_point3(local);
                min = min.min(p);
                max = max.max(p);
            }
        }
        s.local.rect_bounds = Some(super::rect_tool::RectBounds { rotation, min, max });
    });
    settle(rt);
}

#[test]
fn a_rotated_object_resizes_in_its_own_frame_exactly_as_the_rect_shows() {
    let mut rt = open();
    rect_boxes(&mut rt, &[("e_turned", 45.0, [0.0, 0.0, 0.0])]);
    let turned = glam::Quat::from_rotation_y(45f32.to_radians());
    assert!(
        super::rect_tool::frame_rotation(&rt.state().resolved, &["e_turned".into()])
            .dot(turned)
            .abs()
            > 0.9999,
        "one object: the rect is in its own frame"
    );
    let before = rt.state().editor.host.store().revision();
    let shown = drag_rect_edge(&mut rt);
    assert_eq!(
        rt.state().editor.host.store().revision(),
        before + 1,
        "one undo step"
    );
    // The committed box, in the rect's plane, is the rect that showed.
    let world = crate::world::world_transform(&rt.state().resolved, "e_turned").unwrap();
    let (mut min, mut max) = (glam::Vec2::MAX, glam::Vec2::MIN);
    for i in 0..8 {
        let corner = glam::Vec3::new(
            [-0.5, 0.5][i & 1],
            [-0.5, 0.5][(i >> 1) & 1],
            [-0.5, 0.5][(i >> 2) & 1],
        );
        let p = world.transform_point3(corner) - shown.origin;
        let at = glam::Vec2::new(p.dot(shown.axes[0]), p.dot(shown.axes[1]));
        min = min.min(at);
        max = max.max(at);
    }
    assert!(
        min.abs_diff_eq(shown.min, 1e-3) && max.abs_diff_eq(shown.max, 1e-3),
        "committed {min}..{max}, shown {}..{}",
        shown.min,
        shown.max
    );
    let (scale, rotation, _) = world.to_scale_rotation_translation();
    assert!(
        rotation.dot(turned).abs() > 0.9999,
        "rotation kept: {rotation:?}"
    );
    assert!(
        (scale - glam::Vec3::ONE).abs().max_element() > 0.05,
        "it resized: {scale}"
    );
}

#[test]
fn a_mixed_rotation_selection_grows_only_each_objects_axis_nearest_the_drag() {
    let mut rt = open();
    rect_boxes(
        &mut rt,
        &[
            ("e_square", 0.0, [-2.0, 0.0, 0.0]),
            ("e_tilted", 30.0, [2.0, 0.0, 0.0]),
        ],
    );
    let ids: Vec<String> = vec!["e_square".into(), "e_tilted".into()];
    assert_eq!(
        super::rect_tool::frame_rotation(&rt.state().resolved, &ids),
        glam::Quat::IDENTITY,
        "mixed rotations: a world rect"
    );
    let dragged = super::rect_tool::shown(rt.state())
        .expect("the rect shows")
        .axes[0];
    let shown = drag_rect_edge(&mut rt);
    assert!(shown.max.x - shown.min.x > 0.0);
    for (id, turn) in [("e_square", 0f32), ("e_tilted", 30f32)] {
        let world = crate::world::world_transform(&rt.state().resolved, id).unwrap();
        let (scale, rotation, _) = world.to_scale_rotation_translation();
        let start = glam::Quat::from_rotation_y(turn.to_radians());
        assert!(rotation.dot(start).abs() > 0.9999, "{id}: rotation kept");
        let own = [
            start * glam::Vec3::X,
            start * glam::Vec3::Y,
            start * glam::Vec3::Z,
        ];
        let nearest = (0..3)
            .max_by(|&a, &b| {
                own[a]
                    .dot(dragged)
                    .abs()
                    .total_cmp(&own[b].dot(dragged).abs())
            })
            .unwrap();
        for axis in 0..3 {
            match axis == nearest {
                true => assert!(scale[axis] > 1.05, "{id}: its axis {axis} grows: {scale}"),
                false => assert!(
                    (scale[axis] - 1.0).abs() < 1e-4,
                    "{id}: axis {axis} is untouched: {scale}"
                ),
            }
        }
    }
}

/// The box corners of `ids`, in the plane and frame of `shown`, as (min, max).
fn committed_extent(
    rt: &Runtime<Studio>,
    ids: &[&str],
    shown: &super::rect_tool::Frame,
) -> (glam::Vec2, glam::Vec2) {
    let (mut min, mut max) = (glam::Vec2::MAX, glam::Vec2::MIN);
    for id in ids {
        let world = crate::world::world_transform(&rt.state().resolved, id).unwrap();
        for i in 0..8 {
            let corner = glam::Vec3::new(
                [-0.5, 0.5][i & 1],
                [-0.5, 0.5][(i >> 1) & 1],
                [-0.5, 0.5][(i >> 2) & 1],
            );
            let p = world.transform_point3(corner) - shown.origin;
            let at = glam::Vec2::new(p.dot(shown.axes[0]), p.dot(shown.axes[1]));
            min = min.min(at);
            max = max.max(at);
        }
    }
    (min, max)
}

#[test]
fn objects_sharing_a_rotation_resize_as_the_rect_shows() {
    let mut rt = open();
    rect_boxes(
        &mut rt,
        &[
            ("e_left", 45.0, [-2.0, 0.0, 0.0]),
            ("e_right", 45.0, [2.0, 1.0, 1.0]),
        ],
    );
    let ids = ["e_left", "e_right"];
    let turned = glam::Quat::from_rotation_y(45f32.to_radians());
    let frame = super::rect_tool::frame_rotation(&rt.state().resolved, &ids.map(String::from));
    assert!(
        frame.dot(turned).abs() > 0.9999,
        "the rect is in their shared frame"
    );
    let shown = drag_rect_edge(&mut rt);
    let (min, max) = committed_extent(&rt, &ids, &shown);
    assert!(
        min.abs_diff_eq(shown.min, 1e-3) && max.abs_diff_eq(shown.max, 1e-3),
        "committed {min}..{max}, shown {}..{}",
        shown.min,
        shown.max
    );
}

#[test]
fn undoing_a_rect_drag_restores_position_rotation_and_scale() {
    let mut rt = open();
    rect_boxes(&mut rt, &[("e_turned", 45.0, [1.0, 0.0, 0.0])]);
    let before = rt.state().resolved["e_turned"].record["Transform"].clone();
    drag_rect_edge(&mut rt);
    rt.update(|s| s.resolve());
    assert_ne!(rt.state().resolved["e_turned"].record["Transform"], before);
    rt.touch(|s| s.editor.action("history.undo", json!({})).unwrap());
    rt.update(|s| s.resolve());
    assert_eq!(
        rt.state().resolved["e_turned"].record["Transform"],
        before,
        "one undo puts the whole pose back"
    );
}

/// A gesture over the selected `e_turned`, as the Rect tool begins one.
fn rect_gesture(rt: &mut Runtime<Studio>) -> crate::world::gesture::Gesture {
    rt.touch(|s| {
        crate::world::gesture::Gesture::begin_tool(
            &s.resolved,
            &s.editor.state,
            0,
            crate::state::Tool::Rect,
        )
        .unwrap()
    })
}

#[test]
fn a_plain_rect_move_keeps_a_skewed_childs_rotation_and_scale() {
    let mut rt = open();
    rt.touch(|s| {
        s.editor
            .apply(
                vec![
                    json!({"cmd":"entity.create","args":{"id":"e_rig","name":"Rig","components":{"Transform":{"rotation":[0,30,0],"scale":[2,1,0.5]}}}}),
                    json!({"cmd":"entity.create","args":{"id":"e_kid","name":"Kid","parent":"e_rig","components":{"Transform":{"position":[1,0,0],"rotation":[0,20,0]},"MeshRenderer":{"shape":"box"}}}}),
                ],
                "Create",
                None,
            )
            .unwrap();
    });
    rt.update(|s| {
        s.resolve();
        s.select(vec!["e_kid".into()], false);
    });
    let mut gesture = rect_gesture(&mut rt);
    let (start_scale, start_rotation, _) = gesture.initial["e_kid"].to_scale_rotation_translation();
    let parent = crate::world::world_transform(&rt.state().resolved, "e_rig").unwrap();
    let before = crate::world::world_transform(&rt.state().resolved, "e_kid").unwrap();
    let by = glam::Vec3::new(0.5, 0.0, 0.25);
    gesture.rect_resize(
        glam::Mat4::from_translation(by),
        [glam::Vec3::X, glam::Vec3::Y],
        glam::Vec2::ONE,
    );
    let (scale, rotation, _) = gesture.current["e_kid"].to_scale_rotation_translation();
    assert!(
        scale.abs_diff_eq(start_scale, 1e-4),
        "scale kept: {scale} vs {start_scale}"
    );
    assert!(
        rotation.dot(start_rotation).abs() > 0.99999,
        "rotation kept: {rotation:?} vs {start_rotation:?}"
    );
    let after = parent * gesture.current["e_kid"];
    assert!(
        after
            .transform_point3(glam::Vec3::ZERO)
            .abs_diff_eq(before.transform_point3(glam::Vec3::ZERO) + by, 1e-4),
        "it moved by the drag in the world"
    );
}

#[test]
fn align_with_view_moves_a_selected_parent_and_its_selected_child_follows() {
    let mut rt = open();
    rt.touch(|s| {
        s.editor
            .apply(
                vec![json!({"cmd":"entity.create","args":{"id":"e_bulb","name":"Bulb","parent":"e_lamp","components":{"Transform":{"position":[0,1,0]}}}})],
                "Create",
                None,
            )
            .unwrap();
    });
    rt.update(|s| s.resolve());
    assert_eq!(
        rt.state().resolved["e_bulb"].parent.as_deref(),
        Some("e_lamp")
    );
    rt.update(|s| s.select(vec!["e_lamp".into(), "e_bulb".into()], false));
    rt.update(|s| super::commands::run(s, "transform.align_view", &crate::catalog::Target::None));
    settle(&mut rt);
    let camera = rt.state().editor.state.camera;
    let lamp = crate::world::world_transform(&rt.state().resolved, "e_lamp").unwrap();
    assert!(
        lamp.w_axis.truncate().abs_diff_eq(camera.position(), 1e-3),
        "the parent is at the camera"
    );
    let bulb: Vec<f64> = (0..3)
        .map(|i| {
            rt.state().resolved["e_bulb"].record["Transform"]["position"][i]
                .as_f64()
                .unwrap()
        })
        .collect();
    assert_eq!(bulb, [0.0, 1.0, 0.0], "the child keeps its place under it");
}

#[test]
fn a_rect_press_that_has_not_moved_leaves_a_skewed_childs_pose_exactly() {
    let mut rt = open();
    rt.touch(|s| {
        s.editor
            .apply(
                vec![
                    json!({"cmd":"entity.create","args":{"id":"e_rig","name":"Rig","components":{"Transform":{"rotation":[0,30,0],"scale":[2,1,0.5]}}}}),
                    json!({"cmd":"entity.create","args":{"id":"e_kid","name":"Kid","parent":"e_rig","components":{"Transform":{"position":[1,0,0],"rotation":[0,20,0]},"MeshRenderer":{"shape":"box"}}}}),
                ],
                "Create",
                None,
            )
            .unwrap();
    });
    rect_boxes(&mut rt, &[]);
    rt.update(|s| {
        s.select(vec!["e_kid".into()], false);
        s.local.rect_bounds = Some(super::rect_tool::RectBounds {
            rotation: glam::Quat::IDENTITY,
            min: glam::Vec3::splat(-1.0),
            max: glam::Vec3::splat(3.0),
        });
    });
    settle(&mut rt);
    let view = rt.find("scene/view").unwrap();
    let c = super::rect_tool::corners(rt.state(), (view.width(), view.height())).unwrap();
    let inside = (c[0] + c[2]) * 0.5;
    let (x, y) = (view.left + inside.x, view.top + inside.y);
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    rt.pointer_down(x, y);
    rt.pointer_move(x + 6.0, y);
    rt.frame(FRAME);
    rt.pointer_move(x, y);
    rt.frame(FRAME);
    let gesture = rt.state().editor.gesture.as_ref().expect("the drag runs");
    assert_eq!(
        gesture.current["e_kid"], gesture.initial["e_kid"],
        "back where it started: the pose is the one it had, not a re-decomposed one"
    );
}

// The import gate: the card over the window while a project's imports settle.

/// The text a keyed element shows.
fn shown_text(rt: &Runtime<Studio>, key: &str) -> Option<String> {
    rt.tree()
        .into_iter()
        .find(|n| n.key.as_ref().is_some_and(|k| k.to_string() == key))
        .and_then(|n| n.text)
        .map(|t| t.to_string())
}

/// What the import feed says: `done` sources imported, `pending` ids still importing,
/// `failed` ids with their errors.
fn feed(done: usize, pending: &[&str], failed: &[(&str, &str)]) -> ara_play_host::ImportProgress {
    ara_play_host::ImportProgress {
        done,
        total: done + pending.len() + failed.len(),
        pending: pending.iter().map(|id| id.to_string()).collect(),
        failed: failed
            .iter()
            .map(|(id, error)| (id.to_string(), error.to_string()))
            .collect(),
    }
}

/// A gate over the open project whose pipeline reports only what the test feeds it, started
/// `ago` ago.
fn silent_gate(rt: &mut Runtime<Studio>, ago: Duration) -> super::loading::Gate {
    rt.touch(|s| super::loading::Gate::new(&s.editor, false))
        .without_feed()
        .started_ago(ago)
}

#[test]
fn the_gate_follows_a_slow_import_to_its_end() {
    use super::loading::Stage;
    let mut rt = open();
    // Just started and the pipeline silent: held, but the card is not up yet.
    let mut gate = silent_gate(&mut rt, Duration::ZERO);
    let first = gate.step().expect("the gate holds");
    assert!(
        !first.shown,
        "imports that settle at once never show the card"
    );
    assert_eq!(first.stage(), Stage::Preparing);
    // Past the first moments, still silent: the card says it is preparing.
    let mut gate = silent_gate(&mut rt, Duration::from_secs(1));
    let preparing = gate.step().expect("the gate holds");
    assert!(preparing.shown);
    assert_eq!(preparing.stage(), Stage::Preparing);
    assert!(!preparing.skip, "no Open now yet");
    // Three sources importing, then one at a time finishing: the last to finish is named.
    gate.observe(feed(0, &["as_a", "as_b", "as_c"], &[]));
    let importing = gate.step().unwrap();
    assert_eq!(importing.stage(), Stage::Importing);
    let imports = importing.imports.as_ref().unwrap();
    assert_eq!((imports.done, imports.total, imports.pending), (0, 3, 3));
    assert_eq!(importing.latest, None);
    gate.observe(feed(1, &["as_b", "as_c"], &[]));
    let importing = gate.step().unwrap();
    assert_eq!(importing.latest.as_deref(), Some("as_a"));
    assert_eq!(importing.imports.as_ref().unwrap().done, 1);
    // A slow one: Open now appears after a few seconds, and the wait is shown.
    let mut slow = silent_gate(&mut rt, Duration::from_secs(5));
    slow.observe(feed(1, &["as_b", "as_c"], &[]));
    let slow = slow.step().unwrap();
    assert!(slow.skip && slow.elapsed >= 5);
    // Everything imported clean: the gate opens by itself.
    gate.observe(feed(3, &[], &[]));
    assert_eq!(gate.step(), None);
}

#[test]
fn a_failed_import_holds_the_gate_on_a_review_with_its_error() {
    use super::loading::Stage;
    let mut rt = open();
    let mut gate = silent_gate(&mut rt, Duration::from_secs(1));
    gate.observe(feed(
        2,
        &[],
        &[
            (
                "models/b.gltf",
                "ARA4302 the glTF buffer `b.bin` is missing",
            ),
            ("models/a.png", "not an image"),
        ],
    ));
    for _ in 0..3 {
        let review = gate.step().expect("failures keep the gate closed");
        assert_eq!(review.stage(), Stage::Review);
        let failed = &review.imports.as_ref().unwrap().failed;
        assert_eq!(
            failed
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            ["models/a.png", "models/b.gltf"],
            "sorted by path"
        );
        assert!(failed[1].1.contains("ARA4302"), "with the import's error");
    }
}

#[test]
fn a_pipeline_that_never_reports_does_not_hold_the_gate_forever() {
    let mut rt = open();
    let mut gate = silent_gate(&mut rt, Duration::from_secs(13));
    assert_eq!(gate.step(), None);
}

#[test]
fn the_card_shows_progress_and_lets_a_person_in() {
    let mut rt = open();
    // Before the first moments are over the card is not drawn.
    let mut gate = silent_gate(&mut rt, Duration::ZERO);
    let early = gate.step();
    rt.update(|s| s.local.import_gate = early);
    settle(&mut rt);
    assert!(rt.find("loading/detail").is_none(), "no card yet");
    // Importing: the count, the last source, the clock; no Open now before a few seconds.
    let mut gate = silent_gate(&mut rt, Duration::from_secs(2));
    gate.observe(feed(0, &["models/a.gltf", "models/b.gltf"], &[]));
    gate.observe(feed(1, &["models/b.gltf"], &[]));
    let standing = gate.step();
    rt.update(|s| s.local.import_gate = standing);
    settle(&mut rt);
    assert_eq!(
        shown_text(&rt, "loading/detail").as_deref(),
        Some("Last: models/a.gltf")
    );
    assert_eq!(shown_text(&rt, "loading/elapsed").as_deref(), Some("0:02"));
    assert!(rt.find("loading/open").is_none());
    // A few seconds later Open now is there; pressing it lets the person in.
    let mut gate = silent_gate(&mut rt, Duration::from_secs(4));
    gate.observe(feed(1, &["models/b.gltf"], &[]));
    let standing = gate.step();
    rt.update(|s| s.local.import_gate = standing);
    settle(&mut rt);
    press(&mut rt, "loading/open");
    assert!(rt.state().local.import_released);
    settle(&mut rt);
    assert!(rt.find("loading/detail").is_none(), "the card is gone");
}

#[test]
fn the_card_lists_failed_imports_with_their_errors_and_waits_for_open_project() {
    let mut rt = open();
    let mut gate = silent_gate(&mut rt, Duration::from_secs(1));
    let errors: Vec<(String, String)> = (0..5)
        .map(|i| {
            (
                format!("models/bad_{i}.gltf"),
                format!("ARA4302 buffer {i} is missing"),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = errors
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    gate.observe(feed(10, &[], &refs));
    let standing = gate.step();
    rt.update(|s| s.local.import_gate = standing);
    settle(&mut rt);
    let texts: Vec<String> = rt
        .tree()
        .into_iter()
        .filter_map(|n| n.text.map(|t| t.to_string()))
        .collect();
    for i in 0..4 {
        assert!(
            texts.iter().any(|t| t == &format!("models/bad_{i}.gltf")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t == &format!("ARA4302 buffer {i} is missing")),
            "the error of source {i} shows"
        );
    }
    assert!(
        texts.iter().any(|t| t == "and 1 more"),
        "the fifth is counted"
    );
    assert!(texts.iter().any(|t| t == "5 imports failed"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "10 of 15 imported"), "{texts:?}");
    // Review waits for Open project, however long it has been.
    press(&mut rt, "loading/open");
    assert!(rt.state().local.import_released);
}

#[test]
fn native_views_stay_hidden_while_the_gate_holds() {
    let mut rt = open();
    settle(&mut rt);
    assert!(
        super::native_rect(&rt, super::SCENE_LAYER, false).is_some(),
        "the Scene's element has a place"
    );
    for layer in [super::SCENE_LAYER, super::game::GAME_LAYER] {
        assert_eq!(super::native_rect(&rt, layer, true), None, "{layer} hidden");
    }
}

/// A studio over a fresh project whose scene uses `models/crate.gltf`; each of `sources`
/// (file name, importer, bytes) is registered for import. A glTF of `{}` fails to import.
fn open_with_imports(sources: &[(&str, &str, &[u8])]) -> Runtime<Studio> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let root = target.join("studio-tests").join(format!(
        "imports-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let content = root.join(ara_doc::AUTHORING_ROOT);
    std::fs::create_dir_all(content.join("levels")).unwrap();
    std::fs::create_dir_all(content.join("models")).unwrap();
    std::fs::create_dir_all(root.join("settings")).unwrap();
    std::fs::write(
        root.join("settings/ara.imports"),
        ara_asset::ProjectImportSettings::default()
            .canonical_text()
            .unwrap(),
    )
    .unwrap();
    let write = |path: PathBuf, value: Value| {
        std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap()
    };
    write(
        root.join("ara.project.json"),
        json!({"name": "Imports", "layoutVersion": ara_doc::PROJECT_LAYOUT_VERSION, "main": "levels/main.scene"}),
    );
    write(
        content.join("levels/main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_import01","records":{
            "e_crate001":{"name":"Crate","Transform":{},
                "MeshRenderer":{"model":"@models/crate.gltf#as_crate0001"}}}}),
    );
    for (i, (file, importer, bytes)) in sources.iter().enumerate() {
        std::fs::write(content.join("models").join(file), bytes).unwrap();
        write(
            content.join(format!("models/{file}.import.json")),
            json!({"$schema":"ara://schema/import/1","id":format!("as_source{i:04}"),"records":{
                "i_settings":{"importer":importer,"version":"1","settings":{}}}}),
        );
    }
    let editor = Editor::open_with(&root, Some(root.join(".ara/test-user.json"))).unwrap();
    let studio = Studio::new(editor, (1400.0, 900.0));
    let theme = studio.look.theme.clone();
    let mut rt = Runtime::new(studio, chrome::view, theme, (1400, 900), 1.0).unwrap();
    rt.frame(Duration::ZERO);
    rt
}

/// The real pipeline, end to end: a project whose glTF cannot be imported reaches the
/// gate's review with the import's own error, and the gate stays closed on it.
#[test]
fn a_project_with_a_failing_import_reaches_the_review_with_the_real_error() {
    use super::loading::Stage;
    let mut rt = open_with_imports(&[("crate.gltf", "ara.gltf", b"{}")]);
    let mut gate = rt.touch(|s| super::loading::Gate::new(&s.editor, false));
    let started = std::time::Instant::now();
    let review = loop {
        rt.touch(|s| s.editor.tick());
        match gate.step() {
            Some(standing) if standing.stage() == Stage::Review => break standing,
            Some(_) => {}
            None => panic!("the gate opened without a failure or an import report"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the import never settled: {:?}",
            gate.step()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let failed = &review.imports.as_ref().unwrap().failed;
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].0.ends_with("models/crate.gltf"), "{failed:?}");
    assert!(
        !failed[0].1.trim().is_empty(),
        "the import's error is shown"
    );
    assert!(gate.step().is_some(), "a failure keeps the gate closed");
    // The console says it, so a real import can be checked without a screenshot.
    let log = gate.take_log();
    assert!(
        log.iter()
            .any(|l| l.starts_with("imports: review with 1 failure: ")
                && l.contains("models/crate.gltf")),
        "{log:?}"
    );
    // An agent reads the same list over the protocol, and can open the gate.
    let said = rt.touch(|s| s.editor.action("editor.imports", json!({})).unwrap());
    assert_eq!(said["stage"], "review", "{said}");
    assert!(
        said["failed"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("models/crate.gltf")
    );
    assert!(
        !said["failed"][0]["error"]
            .as_str()
            .unwrap()
            .trim()
            .is_empty()
    );
    assert!(!rt.state().editor.import_gate.take_release());
    rt.touch(|s| {
        s.editor
            .action("editor.imports", json!({"release": true}))
            .unwrap()
    });
    assert!(
        rt.state().editor.import_gate.take_release(),
        "release reaches the window"
    );
    gate.release("an agent");
    assert_eq!(gate.step(), None);
    let log = gate.take_log();
    assert_eq!(log, ["imports: opened (an agent), 1 failed"], "{log:?}");
}

/// The real pipeline with no one at the window: the gate opens on the failed import and says
/// what failed, so a studio started for an agent is never held on a review.
#[test]
fn an_unattended_gate_on_a_project_with_a_failing_import_opens_by_itself() {
    let mut rt = open_with_imports(&[("crate.gltf", "ara.gltf", b"{}")]);
    let mut gate = rt.touch(|s| super::loading::Gate::new(&s.editor, true));
    let started = std::time::Instant::now();
    while gate.step().is_some() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the gate never opened"
        );
        rt.touch(|s| s.editor.tick());
        std::thread::sleep(Duration::from_millis(20));
    }
    let log = gate.take_log();
    assert!(
        log.iter()
            .any(|l| l.starts_with("imports: review with 1 failure: ")),
        "{log:?}"
    );
    assert_eq!(
        log.last().map(String::as_str),
        Some("imports: opened (no one at the window), 1 failed"),
        "{log:?}"
    );
}

/// The real pipeline's success path: a valid image imports and the gate opens by itself.
#[test]
fn a_project_whose_imports_succeed_opens_the_gate_by_itself() {
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(4, 4)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    let mut rt = open_with_imports(&[("rock.png", "ara.texture", &png)]);
    let worker =
        ara_play_host::PlayConfig::for_project(rt.state().editor.host.root()).import_worker;
    if !worker.exists() {
        eprintln!("skipped: no ara-import-worker at {}", worker.display());
        return;
    }
    let mut gate = rt.touch(|s| super::loading::Gate::new(&s.editor, false));
    let started = std::time::Instant::now();
    let mut last = None;
    while let Some(standing) = gate.step() {
        last = Some(standing);
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the gate never opened: {last:?}"
        );
        rt.touch(|s| s.editor.tick());
        std::thread::sleep(Duration::from_millis(20));
    }
    // It opened because the import settled clean, not because the pipeline stayed silent.
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "opened by giving up: {last:?}"
    );
    let log = gate.take_log();
    assert_eq!(
        log.last().map(String::as_str),
        Some("imports: opened (all imported), 0 failed"),
        "{log:?}"
    );
}

#[test]
fn an_unattended_gate_opens_when_imports_settle_even_with_failures() {
    let mut rt = open();
    let mut gate = rt
        .touch(|s| super::loading::Gate::new(&s.editor, true))
        .without_feed()
        .started_ago(Duration::from_secs(1));
    let a = (
        "models/a.gltf",
        "ARA4302 missing buffer\nimport-worker exited",
    );
    gate.observe(feed(1, &["models/b.gltf"], &[a]));
    assert!(gate.step().is_some(), "still importing: held");
    gate.observe(feed(1, &[], &[a, ("models/b.gltf", "not a model")]));
    assert_eq!(
        gate.step(),
        None,
        "settled with failures and no one there: open"
    );
    assert_eq!(
        gate.take_log(),
        [
            "imports: importing 1 of 3",
            "imports: review with 2 failures: models/a.gltf: ARA4302 missing buffer; models/b.gltf: not a model",
            "imports: opened (no one at the window), 2 failed",
        ]
    );
}

#[test]
fn an_attended_gate_logs_each_stage_once_and_opens_when_released() {
    let mut rt = open();
    let mut gate = silent_gate(&mut rt, Duration::from_secs(1));
    gate.observe(feed(0, &["a", "b"], &[]));
    for _ in 0..3 {
        gate.step();
    }
    gate.observe(feed(1, &[], &[("b", "bad")]));
    for _ in 0..3 {
        assert!(gate.step().is_some(), "an attended review waits");
    }
    gate.release("a person");
    assert_eq!(gate.step(), None);
    assert_eq!(gate.step(), None);
    assert_eq!(
        gate.take_log(),
        [
            "imports: importing 0 of 2",
            "imports: review with 1 failure: b: bad",
            "imports: opened (a person), 1 failed",
        ],
        "each stage once"
    );
}

#[test]
fn enter_takes_the_cards_button_once_it_is_offered() {
    let mut rt = open();
    // Importing, before Open now is offered: Enter does nothing.
    let mut gate = silent_gate(&mut rt, Duration::from_secs(2));
    gate.observe(feed(0, &["models/a.gltf"], &[]));
    let standing = gate.step();
    rt.update(|s| s.local.import_gate = standing);
    settle(&mut rt);
    rt.key(stillkit::el::KeyCode::ENTER.0);
    assert!(!rt.state().local.import_released, "no button yet");
    // A review: Enter is Open project.
    let mut gate = silent_gate(&mut rt, Duration::from_secs(2));
    gate.observe(feed(1, &[], &[("models/a.gltf", "bad")]));
    let standing = gate.step();
    rt.update(|s| s.local.import_gate = standing);
    settle(&mut rt);
    rt.key(stillkit::el::KeyCode::ENTER.0);
    assert!(rt.state().local.import_released, "Enter opens the project");
}

/// With no one at the window the gate opens past a failed import, and the failure is still in
/// the window afterwards: the Problems panel lists it as an error.
#[test]
fn a_failed_import_is_in_the_problems_panel_after_an_unattended_open() {
    let mut rt = open_with_imports(&[("crate.gltf", "ara.gltf", b"{}")]);
    let mut gate = rt.touch(|s| super::loading::Gate::new(&s.editor, true));
    let started = std::time::Instant::now();
    while gate.step().is_some() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the gate never opened"
        );
        rt.touch(|s| s.editor.tick());
        std::thread::sleep(Duration::from_millis(20));
    }
    // The diagnostics bus carries the failure; the studio reads it as it does each pass.
    let started = std::time::Instant::now();
    while !rt.state().logs.iter().any(|r| {
        r.severity == ara_diag::Severity::Error && r.message.contains("import worker failed")
    }) {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no import error in the logs"
        );
        rt.touch(|s| s.editor.tick());
        rt.touch(Studio::poll_logs);
        std::thread::sleep(Duration::from_millis(20));
    }
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "problems"})));
    settle(&mut rt);
    let texts: Vec<String> = rt
        .tree()
        .into_iter()
        .filter_map(|n| n.text.map(|t| t.to_string()))
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("import worker failed")),
        "the Problems panel lists the failed import: {texts:?}"
    );
}

/// The Assets toolbar at the widths of small windows: the path bar's root step and the search
/// box sit side by side, never one over the other, and the search keeps a usable width where
/// the panel has room for one.
#[test]
fn the_assets_path_bar_and_search_share_the_row_at_narrow_widths() {
    for width in [960_u32, 720, 560, 480] {
        let mut rt = open_sized((width, 600));
        settle(&mut rt);
        let search = rt.find("assets/search").expect("the search box shows");
        let root = rt
            .find("assets/crumb/root")
            .expect("the path bar's root shows");
        assert!(
            root.right <= search.left + 0.5,
            "{width}px: the path bar {root:?} runs under the search box {search:?}"
        );
        assert!(
            width < 720 || search.width() >= 90.0,
            "{width}px: the search box keeps a usable width, {search:?}"
        );
    }
}

/// The type filters are one row (never a wrapped grid), and "Project" is said once in the panel
/// (the tree's root), not again by the path bar or the footer.
#[test]
fn assets_filters_are_one_row_and_project_is_said_once() {
    for width in [1280_u32, 1000] {
        let mut rt = open_sized((width, 720));
        scan(&mut rt);
        settle(&mut rt);
        let tops: std::collections::BTreeSet<i32> = rt
            .tree()
            .into_iter()
            .filter(|n| {
                n.key
                    .as_ref()
                    .is_some_and(|k| k.to_string().starts_with("assets/filter/"))
            })
            .map(|n| n.rect.top as i32)
            .collect();
        assert_eq!(
            tops.len(),
            1,
            "{width}px: the filters share one row, {tops:?}"
        );
        let said: Vec<String> = rt
            .tree()
            .into_iter()
            .filter_map(|n| n.text.map(|t| t.to_string()))
            .filter(|t| t.contains("Project"))
            .collect();
        assert_eq!(
            said,
            ["Project"],
            "{width}px: the root is named once, by the tree"
        );
    }
}

/// The status bar's hint names the launcher chord the title bar's box shows, never the other
/// platform's (it read "⌘K" on Windows).
#[test]
fn the_status_hint_and_the_launcher_box_use_the_same_platform_chord() {
    let hint = super::chrome::idle_hint();
    let chord = super::chrome::launcher_chord();
    assert!(hint.ends_with(&format!("{chord} commands")), "{hint}");
    if cfg!(target_os = "macos") {
        assert!(hint.contains('⌘') && !hint.contains("Ctrl"), "{hint}");
    } else {
        assert!(hint.contains("Ctrl K") && !hint.contains('⌘'), "{hint}");
    }
}

// The UI pass: title bar, panel tabs.

#[test]
fn the_title_bar_has_no_theme_button_and_no_rule_under_it() {
    let mut rt = open();
    settle(&mut rt);
    assert!(rt.find("theme").is_none(), "the theme lives in Settings");
    assert!(rt.find("launcher").is_some() && rt.find("menu/Window").is_some());
    // No hairline between the bar and the panels: the pixel row under the bar is the
    // bar's own surface.
    let bar = rt
        .find("title-bar")
        .or_else(|| rt.find(ara_kit::ui::TITLE_BAR))
        .expect("the bar");
    let shot = rt.snap().unwrap();
    let at = |x: u32, y: u32| {
        let i = ((y * shot.width + x) * 4) as usize;
        shot.pixels[i..i + 4].to_vec()
    };
    let (x, below) = (shot.width / 2, (bar.bottom * rt.scale()) as u32);
    assert_eq!(at(x, below - 1), at(x, below), "one surface, no line");
}

#[test]
fn core_panels_have_no_tab_close_and_the_window_menu_hides_and_shows_them() {
    let mut rt = open();
    settle(&mut rt);
    for panel in ["hierarchy", "inspector", "scene", "game", "assets"] {
        rt.update(|s| s.act("editor.layout.show", json!({"panel": panel})));
        settle(&mut rt);
        assert!(
            rt.find(&format!("tab/{panel}/close")).is_none(),
            "{panel} has no ×"
        );
    }
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "console"})));
    settle(&mut rt);
    assert!(
        rt.find("tab/console/close").is_some(),
        "other panels keep their ×"
    );
    // Window › Hierarchy is checked while shown; pressing it hides the panel, and again shows it.
    for (panel, label) in [("hierarchy", "Hierarchy"), ("assets", "Assets")] {
        for (shown_after, key) in [
            (false, format!("cmd/window.close_panel/{label}")),
            (true, format!("cmd/window.{panel}/{label}")),
        ] {
            press(&mut rt, "menu/Window");
            press(&mut rt, &key);
            assert_eq!(
                crate::state::panel_visible(&rt.state().editor.state, panel),
                shown_after,
                "{label} after pressing {key}"
            );
            settle(&mut rt);
        }
    }
}

/// The left edge of the text `label` in the tree.
fn text_left(rt: &Runtime<Studio>, label: &str) -> f32 {
    rt.tree()
        .into_iter()
        .find(|n| n.text.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("{label} shows"))
        .rect
        .left
}

/// Siblings at one depth line up whatever their names are: a row whose name overflows used to
/// squeeze its own (shrinkable) indent and icon slot, leaving the row whose name fit looking
/// pushed right ("Bulkhead").
#[test]
fn hierarchy_siblings_at_one_depth_start_their_names_at_the_same_x() {
    let mut rt = open_sized((1000, 630));
    let long = "Outpost floor with a name much longer than the row";
    rt.touch(|s| {
        s.editor
            .apply(
                vec![
                    json!({"cmd":"entity.create","args":{"id":"e_long","name":long,"components":{"Transform":{}}}}),
                    json!({"cmd":"entity.create","args":{"id":"e_bulk","name":"Bulkhead","components":{"Transform":{}}}}),
                ],
                "Create",
                None,
            )
            .unwrap()
    });
    rt.update(|s| s.resolve());
    settle(&mut rt);
    let bulk = text_left(&rt, "Bulkhead");
    let lamp = text_left(&rt, "Lamp");
    let overflowing = rt
        .tree()
        .into_iter()
        .find(|n| {
            n.text
                .as_deref()
                .is_some_and(|t| t.starts_with("Outpost floor"))
        })
        .expect("the long name shows (cut at the edge)")
        .rect
        .left;
    assert_eq!(bulk, lamp, "Bulkhead lines up with Lamp");
    assert_eq!(
        overflowing, lamp,
        "an overflowing name starts where the others do"
    );
}

/// Names use the row's width and end only at its real edge (past the visibility and lock
/// toggles a row only reserves room for while they are set or the pointer is on it).
#[test]
fn hierarchy_names_run_to_the_row_edge() {
    let mut rt = open_sized((1000, 630));
    rt.touch(|s| {
        s.editor
            .apply(
                vec![json!({"cmd":"entity.create","args":{"id":"e_long","name":"A very long entity name that has to be cut somewhere","components":{"Transform":{}}}})],
                "Create",
                None,
            )
            .unwrap()
    });
    rt.update(|s| s.resolve());
    settle(&mut rt);
    let row = rt.find("row/e_long").expect("the row");
    let name = rt
        .tree()
        .into_iter()
        .find(|n| {
            n.text
                .as_deref()
                .is_some_and(|t| t.starts_with("A very long"))
        })
        .expect("the name")
        .rect;
    assert!(
        row.right - name.right <= 8.0,
        "the name ends {}px short of the row's edge: {name:?} in {row:?}",
        row.right - name.right
    );
}

// The second UI pass: tiles in a short panel, filter names, the path bar, the stats readout.

#[test]
fn tiles_step_down_to_fit_a_short_panel_and_never_lose_their_name() {
    use super::panels::{TileSize, fitting};
    // No measurement yet: the chosen size.
    assert_eq!(fitting(TileSize::Medium, None), TileSize::Medium);
    // Plenty of room keeps the choice; less steps down; too little is the list.
    assert_eq!(fitting(TileSize::Large, Some(400.0)), TileSize::Large);
    assert_eq!(fitting(TileSize::Large, Some(110.0)), TileSize::Medium);
    assert_eq!(fitting(TileSize::Medium, Some(95.0)), TileSize::Small);
    assert_eq!(fitting(TileSize::Medium, Some(77.0)), TileSize::List);
    assert_eq!(fitting(TileSize::List, Some(500.0)), TileSize::List);
    // The real panel: with a body too short for a tile, the folder is a list row with its name.
    let mut rt = open_sized((1000, 630));
    scan(&mut rt);
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
    settle(&mut rt);
    let body = rt.find("assets/body").expect("the Assets body");
    let height_of = |rt: &Runtime<Studio>| {
        rt.tree()
            .into_iter()
            .find(|n| {
                n.key
                    .as_ref()
                    .is_some_and(|k| k.to_string().starts_with("asset/"))
            })
            .map(|n| n.rect.height())
            .expect("an asset shows")
    };
    // Tall: tiles. Short (the measured height the host feeds back): a list row.
    rt.update(|s| {
        s.local.rects.insert(
            "assets/body".into(),
            stillkit::Rect::new(body.left, body.top, body.width(), 400.0),
        );
    });
    settle(&mut rt);
    assert!(height_of(&rt) > 60.0, "tiles when there is room");
    rt.update(|s| {
        s.local.rects.insert(
            "assets/body".into(),
            stillkit::Rect::new(body.left, body.top, body.width(), 62.0),
        );
    });
    settle(&mut rt);
    assert_eq!(height_of(&rt), 22.0, "a list row in a short panel");
    let named = rt.tree().into_iter().any(|n| {
        n.key
            .as_ref()
            .is_some_and(|k| k.to_string().starts_with("asset/"))
            && n.label.is_some()
    });
    assert!(named, "and it carries its name");
}

#[test]
fn each_asset_filter_is_named_and_the_active_one_shows_its_name() {
    let mut rt = open_sized((1000, 630));
    scan(&mut rt);
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
    settle(&mut rt);
    let chips: Vec<(String, Option<String>)> = rt
        .tree()
        .into_iter()
        .filter(|n| {
            n.key
                .as_ref()
                .is_some_and(|k| k.to_string().starts_with("assets/filter/"))
        })
        .map(|n| {
            (
                n.key.unwrap().to_string(),
                n.label.as_deref().map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(chips.len(), 9, "{chips:?}");
    assert!(
        chips
            .iter()
            .all(|(_, label)| label.as_deref().is_some_and(|l| !l.is_empty())),
        "{chips:?}"
    );
    press(&mut rt, "assets/filter/model");
    let shown: Vec<String> = rt
        .tree()
        .into_iter()
        .filter_map(|n| n.text.map(|t| t.to_string()))
        .collect();
    assert!(
        shown.iter().any(|t| t == "Models"),
        "the active filter shows its name: {shown:?}"
    );
}

#[test]
fn the_path_bar_reads_as_a_path_from_the_project_down() {
    let mut rt = open_sized((1000, 630));
    scan(&mut rt);
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
    settle(&mut rt);
    let root = rt.find("assets/crumb/root").expect("the root step");
    let search = rt.find("assets/search").expect("the search");
    let on_bar = |rt: &Runtime<Studio>, label: &str| {
        rt.tree().into_iter().any(|n| {
            n.text.as_deref() == Some(label)
                && (n.rect.top - search.top).abs() < 12.0
                && n.rect.right <= search.left + 0.5
        })
    };
    // The project's name (its marker's `name`, else its folder's) is the root step's label.
    let project = rt
        .tree()
        .into_iter()
        .find(|n| {
            n.key
                .as_ref()
                .is_some_and(|k| k.to_string() == "assets/crumb/root")
        })
        .and_then(|n| n.label.as_deref().map(str::to_owned))
        .expect("the root step is named");
    assert_ne!(project, "Project");
    assert!(
        on_bar(&rt, &project),
        "the root names the project, not \"Project\": {root:?}"
    );
    // Inside a folder: the project, then the folder's own name.
    rt.update(|s| {
        s.act(
            "editor.workspace",
            json!({"folder": ara_doc::AUTHORING_ROOT, "asset": ""}),
        )
    });
    settle(&mut rt);
    assert!(
        on_bar(&rt, &project) && on_bar(&rt, ara_doc::AUTHORING_ROOT),
        "project › {}",
        ara_doc::AUTHORING_ROOT
    );
}

#[test]
fn the_frame_time_readout_is_scene_statistics_and_off_by_default() {
    let mut rt = open();
    rt.update(|s| {
        s.local.scene.ready = true;
        s.local.scene.fps = Some((2.0, 631.0));
    });
    settle(&mut rt);
    let shows = |rt: &Runtime<Studio>| {
        rt.tree()
            .into_iter()
            .any(|n| n.text.as_deref().is_some_and(|t| t.contains(" fps ")))
    };
    assert!(!shows(&rt), "no \"2 fps 631 ms\" unless asked for");
    rt.update(|s| {
        s.act(
            "editor.settings.set",
            json!({"key": "viewport.stats", "value": true}),
        )
    });
    settle(&mut rt);
    assert!(shows(&rt), "Scene statistics shows it");
}

// UI pass 3: the left of the window.

/// How far from its panel's left edge things start, at 1000x630 (logical px). Before: a
/// Hierarchy entity's name 50, "Project" 45 and "content" 57 in the Assets tree.
#[test]
fn the_left_columns_start_their_names_close_to_the_edge() {
    let mut rt = open_sized((1000, 630));
    scan(&mut rt);
    rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
    settle(&mut rt);
    let tree = rt.tree();
    let left_of = |label: &str| {
        tree.iter()
            .filter(|n| n.text.as_deref() == Some(label))
            .map(|n| n.rect.left)
            .fold(f32::MAX, f32::min)
    };
    let area = |key: &str| {
        tree.iter()
            .find(|n| n.key.as_ref().is_some_and(|k| k.to_string() == key))
            .map(|n| n.rect)
            .unwrap_or_else(|| panic!("{key} shows"))
    };
    let outline = area("area/outline");
    let name = left_of("Box") - outline.left;
    assert!(
        name <= 36.0,
        "a Hierarchy entity's name starts {name}px from its panel's edge"
    );
    let tree_edge = tree
        .iter()
        .filter(|n| {
            n.key
                .as_ref()
                .is_some_and(|k| k.to_string().starts_with("assets/tree/"))
        })
        .map(|n| n.rect.left)
        .fold(f32::MAX, f32::min)
        - 3.0;
    let project = left_of("Project") - tree_edge;
    let content = left_of("content") - tree_edge;
    assert!(
        project <= 24.0,
        "\"Project\" starts {project}px from its column's edge"
    );
    assert!(
        content <= 38.0,
        "\"content\" starts {content}px from its column's edge"
    );
    // The left column is narrower by default: the Hierarchy takes at most 14% of the window.
    assert!(
        outline.width() <= 1000.0 * 0.14,
        "the Hierarchy is {}px wide",
        outline.width()
    );
}

/// The UI pass 3 review shots at three window sizes (device px; the zoom applies as in the
/// window): a three-level Hierarchy nest, the Assets tree two folders deep with the path bar and a
/// hovered filter's tooltip, and Settings' theme control. Written to
/// `target/studio-captures/pass3/<ARA_CAP_TAG, default "after">/`.
#[test]
#[ignore = "writes review captures under target/"]
fn pass_three_captures() {
    let tag = std::env::var("ARA_CAP_TAG").unwrap_or_else(|_| "after".into());
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/studio-captures/pass3")
        .join(tag);
    std::fs::create_dir_all(&out).unwrap();
    let shot = |rt: &mut Runtime<Studio>, name: &str| {
        std::fs::write(
            out.join(format!("{name}.png")),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
    };
    for (w, h) in [(1280u32, 720u32), (1000, 630), (900, 560)] {
        let zoom = super::zoom::for_width(w as f32);
        let mut rt = open_sized(((w as f32 / zoom) as u32, (h as f32 / zoom) as u32));
        rt.set_scale(zoom);
        rt.resize((w, h));
        let size = format!("{w}x{h}");
        let root = rt.state().editor.host.root().to_owned();
        std::fs::create_dir_all(root.join("content/models/props")).unwrap();
        std::fs::create_dir_all(root.join("content/textures")).unwrap();
        std::fs::write(root.join("content/models/props/notes.md"), "# Props").unwrap();
        rt.touch(|s| {
            s.editor
                .apply(
                    vec![
                        json!({"cmd":"entity.create","args":{"id":"e_arm","name":"Arm","parent":"e_lamp","components":{"Transform":{}}}}),
                        json!({"cmd":"entity.create","args":{"id":"e_hand","name":"Hand","parent":"e_arm","components":{"Transform":{}}}}),
                    ],
                    "Create",
                    None,
                )
                .unwrap()
        });
        rt.update(|s| s.resolve());
        scan(&mut rt);
        rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
        settle(&mut rt);
        for _ in 0..20 {
            rt.frame(FRAME);
        }
        // The host feeds back the Assets body and path bar rects; do the same here.
        let measure = |rt: &mut Runtime<Studio>| {
            let scale = rt.scale();
            let rects: Vec<_> = ["assets/body", "assets/crumbs", "assets/search"]
                .into_iter()
                .filter_map(|k| rt.find(k).map(|r| (k.to_owned(), r)))
                .map(|(k, r)| {
                    let r = stillkit::Rect::new(
                        r.left / scale,
                        r.top / scale,
                        r.width() / scale,
                        r.height() / scale,
                    );
                    (k, r)
                })
                .collect();
            rt.update(|s| s.local.rects.extend(rects));
            settle(rt);
        };
        measure(&mut rt);
        shot(&mut rt, &format!("{size}-hierarchy-nest"));
        // Two folders deep: the path bar reads project › content › models › props.
        rt.update(|s| {
            s.act(
                "editor.workspace",
                json!({"folder": "content/models/props", "asset": ""}),
            )
        });
        settle(&mut rt);
        measure(&mut rt);
        shot(&mut rt, &format!("{size}-assets-two-deep"));
        // A hovered filter shows its name in a tooltip once the pointer has rested.
        let chip = rt
            .find("assets/filter/texture")
            .expect("the Textures filter");
        rt.pointer_move(chip.center().x, chip.center().y);
        for _ in 0..80 {
            rt.frame(FRAME);
        }
        assert!(
            rt.tree()
                .iter()
                .any(|n| n.text.as_deref() == Some("Textures")),
            "the hovered filter's tooltip shows"
        );
        shot(&mut rt, &format!("{size}-assets-filter-tooltip"));
        rt.pointer_move(0.0, 0.0);
        // Exit with a number mid-drag asks first.
        rt.update(|s| s.local.scrub = Some(("Transform.position".into(), Some(0), 0.0, 2.0)));
        rt.update(|s| super::commands::run(s, "file.exit", &crate::catalog::Target::None));
        settle(&mut rt);
        assert!(rt.find("quit/save").is_some(), "the question shows");
        for _ in 0..40 {
            rt.frame(FRAME);
        }
        shot(&mut rt, &format!("{size}-quit-dialog"));
        press(&mut rt, "quit/cancel");
        rt.update(|s| s.local.scrub = None);
        // Settings, filtered to the theme control.
        rt.update(|s| super::commands::run(s, "edit.preferences", &crate::catalog::Target::None));
        settle(&mut rt);
        rt.update(|s| s.local.settings.as_mut().unwrap().query = "Theme".into());
        settle(&mut rt);
        shot(&mut rt, &format!("{size}-settings-theme"));
    }
}

/// The path bar names as many trailing folders as fit and folds the rest into one "…" step
/// that opens the nearest hidden folder; it never overprints.
#[test]
fn a_deep_path_folds_its_leading_folders_into_one_step() {
    use super::panels::crumbs_fit;
    let parts = ["models", "props", "crates", "small"];
    let fit = |parts: &[&str], room| {
        let c = crumbs_fit("Studio", parts, room);
        (c.shown, c.root_named)
    };
    assert_eq!(fit(&parts, None), (4, true), "unmeasured shows all");
    assert_eq!(fit(&parts, Some(900.0)), (4, true), "room for all");
    let (some, named) = fit(&parts, Some(260.0));
    assert!(
        (1..4).contains(&some) && named,
        "{some} of 4 in a narrow bar"
    );
    assert_eq!(
        fit(&parts, Some(10.0)),
        (1, false),
        "the current one stays, the root is its icon"
    );
    assert_eq!(fit(&[], Some(60.0)), (0, false), "no room for the name");
    assert_eq!(fit(&[], Some(200.0)), (0, true), "the root alone");
    // What it reserves is what it shows.
    let c = crumbs_fit("Studio", &parts, Some(260.0));
    assert!(c.width <= 260.0, "{} fits 260", c.width);
}

#[test]
fn exit_asks_only_when_work_could_be_lost_and_a_tool_shutdown_never_asks() {
    use crate::catalog::Target;
    let exit = |rt: &mut Runtime<Studio>| {
        rt.update(|s| super::commands::run(s, "file.exit", &Target::None));
        settle(rt);
    };
    // Nothing at risk: it closes at once.
    let mut rt = open();
    rt.update(|s| s.editor.host.flush());
    exit(&mut rt);
    assert!(rt.state().local.quit, "closes without asking");
    assert!(rt.state().local.quit_ask.is_none());

    // A number being dragged: it asks first, and Cancel keeps the editor open.
    let mut rt = open();
    rt.update(|s| s.local.scrub = Some(("Transform.position".into(), Some(0), 0.0, 2.0)));
    exit(&mut rt);
    assert!(!rt.state().local.quit, "asks first");
    assert!(rt.state().local.quit_ask.is_some());
    assert!(rt.find("quit/save").is_some() && rt.find("quit/discard").is_some());
    press(&mut rt, "quit/cancel");
    assert!(rt.state().local.quit_ask.is_none() && !rt.state().local.quit);

    // Save and close commits the drag, then closes.
    exit(&mut rt);
    press(&mut rt, "quit/save");
    assert!(rt.state().local.quit);
    assert_eq!(
        position(&rt, "e_box")[0].as_f64(),
        Some(2.0),
        "the drag's value was committed"
    );

    // Discard and close drops the drag.
    let mut rt = open();
    rt.update(|s| s.local.scrub = Some(("Transform.position".into(), Some(0), 0.0, 2.0)));
    exit(&mut rt);
    press(&mut rt, "quit/discard");
    assert!(rt.state().local.quit && rt.state().local.scrub.is_none());
    assert_eq!(position(&rt, "e_box")[0].as_f64(), Some(0.0));
}

/// The shared placeholder for what the renderer does not provide yet shows its one line, in a
/// tile that fills the space it is given. Also writes `target/studio-captures/pass3/unavailable.png`.
#[test]
fn the_unavailable_tile_says_what_is_missing() {
    let studio = open_studio((320, 320));
    let theme = studio.look.theme.clone();
    let mut rt = Runtime::new(
        studio,
        |s: &Studio| {
            stillkit::el::col()
                .size(320.0)
                .fill(s.look.c.card.0)
                .child(super::unavailable::tile(
                    &s.look,
                    "unavailable",
                    "Material previews are not available yet.",
                ))
        },
        theme,
        (320, 320),
        1.0,
    )
    .unwrap();
    settle(&mut rt);
    let tile = rt.find("unavailable").expect("the tile shows");
    assert!(
        tile.width() > 300.0 && tile.height() > 300.0,
        "it fills its parent"
    );
    assert!(
        rt.tree()
            .iter()
            .any(|n| n.text.as_deref() == Some("Material previews are not available yet.")),
        "and says what is missing"
    );
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/studio-captures/pass3");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        out.join("unavailable.png"),
        ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
    )
    .unwrap();
}

/// The narrow default times the small-window zoom must not cut a child's name: at 900x560 (a
/// 818 px wide layout) the Hierarchy still fits about 14 characters of an indented name.
#[test]
fn the_hierarchy_keeps_room_for_an_indented_name_in_a_small_window() {
    let mut rt = open_sized((818, 509));
    rt.touch(|s| {
        s.editor
            .apply(
                vec![
                    json!({"cmd":"entity.create","args":{"id":"e_ter","name":"Terrain","components":{"Transform":{}}}}),
                    json!({"cmd":"entity.create","args":{"id":"e_chunk","name":"Terrain chunk","parent":"e_ter","components":{"Transform":{}}}}),
                ],
                "Create",
                None,
            )
            .unwrap()
    });
    rt.update(|s| s.resolve());
    settle(&mut rt);
    let outline = rt.find("area/outline").expect("the Hierarchy area");
    assert!(outline.width() >= 136.0, "{}px wide", outline.width());
    let name = rt
        .tree()
        .into_iter()
        .find(|n| n.text.as_deref() == Some("Terrain chunk"))
        .expect("the child's name shows");
    assert!(
        name.rect.right <= outline.right && name.rect.width() >= 13.0 * 5.5,
        "the name is {}px wide inside a {}px panel",
        name.rect.width(),
        outline.width()
    );
}

/// A material with UV, noise, colour and world-space operations: the material page tests' showcase.
const SHOWCASE: &str = "material Paint : surface budget 1 { param color : ColorLinear = linear(0.18, 0.42, 0.72); param roughness : float = 0.35; let tiling : vec2 = uv(); let scale : float = 6; let scaled : vec2 = mul(tiling, scale); let grain : float = noise(scaled); let dark : ColorLinear = linear(0.03, 0.05, 0.08); let tinted : ColorLinear = lerp(dark, color, grain); let n : vec3 = worldNormal(); let v : vec3 = viewDirection(); let power : float = 3; let rim : float = fresnel(n, v, power); output baseColor = tinted; output roughness = roughness; output clearCoat = rim; }";

#[test]
fn the_game_view_outside_play_says_its_preview_is_not_available() {
    let mut rt = open();
    rt.update(|s| {
        s.local.game.camera = true;
        s.act("editor.layout.show", json!({"panel": "game"}));
    });
    settle(&mut rt);
    assert!(
        rt.find("game/preview-unavailable").is_some(),
        "the tile shows"
    );
    assert!(
        rt.tree().iter().any(|n| n
            .text
            .as_deref()
            .is_some_and(|t| t.starts_with("The Game preview is not available yet"))),
        "and says what is missing"
    );
}

/// Review shots of each placeholder that stands in for the old renderer's output, at 1280x720
/// and 1000x630 (zoom applied as in the window): the Game preview, the VFX page, the Material
/// page's Details, the Lighting page, the disabled draw modes and effects, and Assets tiles
/// without pictures for models and materials. Written to `target/studio-captures/previews/`.
#[test]
#[ignore = "writes review captures under target/"]
fn placeholder_captures() {
    let out =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/studio-captures/previews");
    std::fs::create_dir_all(&out).unwrap();
    for (w, h) in [(1280u32, 720u32), (1000, 630)] {
        let zoom = super::zoom::for_width(w as f32);
        let size = format!("{w}x{h}");
        let shoot = |rt: &mut Runtime<Studio>, name: &str| {
            for _ in 0..30 {
                rt.frame(FRAME);
            }
            std::fs::write(
                out.join(format!("{size}-{name}.png")),
                ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
            )
            .unwrap();
        };
        let fresh = || {
            let mut rt = open_sized(((w as f32 / zoom) as u32, (h as f32 / zoom) as u32));
            rt.set_scale(zoom);
            rt.resize((w, h));
            rt
        };
        // The Game view outside Play.
        let mut rt = fresh();
        rt.update(|s| {
            s.local.game.camera = true;
            s.act("editor.layout.show", json!({"panel": "game"}));
        });
        shoot(&mut rt, "game-preview");
        // The draw-mode menu: modes and effects the renderer lacks are disabled.
        let mut rt = fresh();
        host_feedback(&mut rt);
        if rt.find("scene/draw-mode").is_some() {
            press(&mut rt, "scene/draw-mode");
        } else {
            // A narrow window folds the Scene's view group; the command opens the menu anyway.
            rt.update(|s| {
                super::commands::run(s, "window.render_debug", &crate::catalog::Target::None)
            });
        }
        shoot(&mut rt, "draw-modes");
        // The VFX page.
        let mut rt = fresh();
        rt.touch(|s| {
            let doc = s
                .editor
                .action(
                    "editor.resource.create",
                    json!({"path": "samples/fx.graph", "domain": "vfx"}),
                )
                .unwrap()["doc"]
                .as_str()
                .unwrap()
                .to_owned();
            s.act("editor.workspace", json!({"resource": doc}));
        });
        rt.update(|s| super::commands::run(s, "window.vfx", &crate::catalog::Target::None));
        shoot(&mut rt, "vfx");
        // The Material page's Details.
        let mut rt = fresh();
        rt.touch(|s| {
            let doc = s
                .editor
                .action(
                    "editor.resource.create",
                    json!({"path": "samples/paint.graph", "domain": "material"}),
                )
                .unwrap()["doc"]
                .as_str()
                .unwrap()
                .to_owned();
            s.editor
                .apply_to(
                    &doc,
                    vec![json!({"cmd": "material.text.apply", "args": {"text": SHOWCASE}})],
                    "Showcase",
                )
                .unwrap();
            s.act("editor.workspace", json!({"resource": doc}));
        });
        rt.update(|s| super::commands::run(s, "window.material", &crate::catalog::Target::None));
        rt.update(|s| super::graph::sync(s, "material"));
        shoot(&mut rt, "material");
        // The Lighting page with a Reflection Probe.
        let mut rt = fresh();
        rt.update(|s| super::commands::run(s, "create.light.probe", &crate::catalog::Target::None));
        rt.update(|s| s.act("editor.layout.show", json!({"panel": "lighting"})));
        shoot(&mut rt, "lighting");
        // Assets: a texture gets its picture; models and materials keep their glyph.
        let mut rt = fresh();
        let root = rt.state().editor.host.root().to_owned();
        let content = root.join(ara_doc::AUTHORING_ROOT);
        std::fs::create_dir_all(content.join("kit")).unwrap();
        image::RgbaImage::from_fn(64, 64, |x, y| {
            image::Rgba([(x * 4) as u8, (y * 4) as u8, 160, 255])
        })
        .save(content.join("kit/panel.png"))
        .unwrap();
        std::fs::write(
            content.join("kit/crate.gltf"),
            r#"{"asset":{"version":"2.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            content.join("kit/paint.material"),
            "material Paint : surface { }",
        )
        .unwrap();
        scan(&mut rt);
        rt.update(|s| s.act("editor.layout.show", json!({"panel": "assets"})));
        rt.update(|s| {
            s.act(
                "editor.workspace",
                json!({"folder": "content/kit", "asset": ""}),
            )
        });
        settle(&mut rt);
        rt.update(|s| s.previews.want(&root, ["kit/panel.png"]));
        for _ in 0..100 {
            rt.update(|s| {
                s.previews.poll();
            });
            std::thread::sleep(Duration::from_millis(10));
        }
        // The host feeds back the Assets body's size, so tiles step down to fit it.
        let scale = rt.scale();
        if let Some(body) = rt.find("assets/body") {
            let body = stillkit::Rect::new(
                body.left / scale,
                body.top / scale,
                body.width() / scale,
                body.height() / scale,
            );
            rt.update(|s| {
                s.local.rects.insert("assets/body".into(), body);
            });
        }
        shoot(&mut rt, "assets-tiles");
    }
}

/// The title bar's parts (device px) in a Windows window `width` logical px wide on a display
/// scaled by `platform`: drawn caption buttons, no inset, and the zoom the window would use.
fn title_bar_parts(width: u32, platform: f32) -> (Vec<(String, stillkit::Rect)>, f32) {
    let zoom = super::zoom::for_width(width as f32);
    let scale = platform * zoom;
    let device = (width as f32 * platform) as u32;
    let height = (632.0 * platform) as u32;
    let mut studio = open_studio((
        (device as f32 / scale) as u32,
        (height as f32 / scale) as u32,
    ));
    // A long name, as a project can have: wider text must never push a caption button out.
    studio.editor.state.settings.insert(
        "project.name".into(),
        json!("An Unreasonably Long Project Name"),
    );
    studio.frame = super::state::Frame {
        inset: 0.0,
        controls: Some(false),
    };
    let theme = studio.look.theme.clone();
    let mut rt = Runtime::new(studio, chrome::view, theme, (device, height), scale).unwrap();
    settle(&mut rt);
    let keys = [
        "title/logo",
        "menu/File",
        "menu/Edit",
        "menu/View",
        "menu/Create",
        "menu/Window",
        "menu/Help",
        "title/play",
        "title/project",
        "launcher",
        "window/minimize",
        "window/maximize",
        "window/close",
    ];
    let parts = keys
        .iter()
        .filter_map(|k| rt.find(k).map(|r| ((*k).to_owned(), r)))
        .collect();
    (parts, device as f32)
}

/// At every window width the studio can have (below and above the zoom step, the minimum
/// window, 125% and 150% displays), the title bar's parts lie inside the window and apart, in
/// order. The order of sacrifice is explicit: the project name collapses first, then the search
/// box to its icon; the logo, menus, Play controls and the three caption buttons never give way
/// (a window must stay closable, maximizable and draggable).
#[test]
fn the_title_bar_fits_the_window_at_every_zoom_step() {
    let minimum = super::MIN_WINDOW.0 - 16; // Windows' client area inside the minimum window
    for platform in [1.0f32, 1.25, 1.5] {
        for width in [minimum, 1024, 1099, 1100, 1280, 1600] {
            let (parts, w) = title_bar_parts(width, platform);
            let at = format!("{width}px at {platform}x");
            let by = |k: &str| parts.iter().find(|(n, _)| n == k).map(|(_, r)| *r);
            for always in [
                "title/logo",
                "menu/File",
                "menu/Help",
                "title/play",
                "window/minimize",
                "window/maximize",
                "window/close",
            ] {
                assert!(
                    by(always).is_some_and(|r| r.width() > 8.0),
                    "{always} shows at {at}"
                );
            }
            // Present parts (a collapsed name or search has no width) lie inside the window.
            let shown: Vec<_> = parts.iter().filter(|(_, r)| r.width() >= 1.0).collect();
            for (name, r) in &shown {
                assert!(
                    r.left >= -0.5 && r.right <= w + 0.5,
                    "{name} {r:?} is outside the window at {at}"
                );
            }
            for (i, (a, ra)) in shown.iter().enumerate() {
                for (b, rb) in shown.iter().skip(i + 1) {
                    assert!(
                        ra.right <= rb.left + 0.5 || rb.right <= ra.left + 0.5,
                        "{a} {ra:?} overlaps {b} {rb:?} at {at}"
                    );
                }
            }
            // The caption buttons keep the window's right edge, the logo its left.
            let close = by("window/close").unwrap();
            assert!(w - close.right < 1.5, "close is at the edge at {at}");
            assert!(
                by("title/logo").unwrap().left >= 5.0,
                "the logo is on the window at {at}"
            );
            // At the minimum, the name and search have given way before any caption button.
            if width == minimum {
                assert!(by("window/maximize").is_some() && by("window/minimize").is_some());
            }
        }
    }
}

/// The Scene's frame is extracted from the world by a free function that holds no renderer, and
/// it is the frame the content store builds: the same draws, in the same order, with the
/// editor's camera in it.
#[test]
fn the_scene_frame_is_extracted_from_the_world_without_a_renderer() {
    use crate::world::gltf::{FidelityContent, FrameInputs, extract_frame};
    let rt = open();
    let s = rt.state();
    let view = s.editor.view().unwrap();
    let store = s.editor.host.store();
    let hidden = std::collections::BTreeSet::new();
    let mut content = FidelityContent::new(s.editor.host.root()).unwrap();
    let frame = extract_frame(
        &mut content,
        FrameInputs {
            view: &view,
            workspace: store.workspace(),
            schema: store.schema(),
            poses: None,
            hidden: &hidden,
            camera: &s.editor.state.camera,
        },
    )
    .unwrap();
    let mut other = FidelityContent::new(s.editor.host.root()).unwrap();
    let direct = other
        .frame_with_poses(&view, store.workspace(), store.schema(), None, &hidden)
        .unwrap();
    assert!(!frame.draws.is_empty(), "the Box draws");
    assert_eq!(
        format!("{:?}", frame.draws),
        format!("{:?}", direct.draws),
        "identical draws"
    );
    assert!(frame.camera.is_some(), "the editor's camera is the frame's");
}
