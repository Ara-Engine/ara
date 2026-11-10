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
    let studio = Studio::new(editor, (1400.0, 900.0));
    let theme = studio.look.theme.clone();
    let mut rt = Runtime::new(studio, chrome::view, theme, (1400, 900), 1.0).unwrap();
    rt.frame(Duration::ZERO);
    rt
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
fn the_draw_mode_menu_switches_view_modes_and_effects() {
    let mut rt = open();
    settle(&mut rt);
    press(&mut rt, "scene/draw-mode");
    assert!(
        rt.find("scene/draw-mode/overdraw").is_some(),
        "every mode is offered"
    );
    press(&mut rt, "scene/draw-mode/wireframe");
    assert_eq!(rt.state().editor.state.view_mode, "wireframe");
    press(&mut rt, "scene/draw-mode");
    press(&mut rt, "scene/effect/Shadows");
    assert!(
        !rt.state().editor.state.render_debug.shadows,
        "Shadows turned off"
    );
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
        for (id, panel) in [("float-a", "console"), ("float-b", "problems")] {
            s.editor.state.floating.push(ara_ui::docking::Floating {
                id: id.into(),
                rect: [40, 40, 400, 300],
                root: ara_ui::docking::DockNode::Area {
                    id: format!("{id}-area"),
                    tabs: vec![panel.into()],
                    active: 0,
                },
            });
        }
    });
    run(
        &mut rt,
        "window.focus_window",
        Target::Window("float-a".into()),
    );
    let order: Vec<_> = rt
        .state()
        .editor
        .state
        .floating
        .iter()
        .map(|f| f.id.clone())
        .collect();
    assert_eq!(order, ["float-b", "float-a"]);
    // Commands not built yet say why instead of failing.
    run(&mut rt, "file.save_as", Target::None);
    assert!(
        rt.state()
            .local
            .note
            .as_ref()
            .is_some_and(|(n, _)| n.starts_with("Save As"))
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
    let scene = crate::app::viewport::SceneView::default();
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
        let diagnostic = match severity {
            Severity::Error => ara_diag::Diagnostic::error(ara_diag::Code(seq as u16), message),
            Severity::Warning => ara_diag::Diagnostic::warning(ara_diag::Code(seq as u16), message),
            _ => ara_diag::Diagnostic::info(ara_diag::Code(seq as u16), message),
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
            "ARA0001",
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
    // Exit asks the host to close.
    run(&mut rt, "file.exit");
    assert!(rt.state().local.quit);
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
    run(&mut rt, "view.mode.lit_wireframe");
    assert_eq!(rt.state().editor.state.view_mode, "litWireframe");
    run(&mut rt, "view.debug.shadows");
    assert!(!rt.state().editor.state.render_debug.shadows);
    run(&mut rt, "view.debug.reset");
    assert!(rt.state().editor.state.render_debug.shadows);
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
        code: "ARA0001".into(),
        message: message.into(),
        origin: "engine".into(),
        at: 0,
        source: None,
        count: 1,
        diagnostic: std::sync::Arc::new(ara_diag::Diagnostic::warning(ara_diag::Code(1), message)),
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
                ara_diag::Diagnostic::warning(ara_diag::Code(1), "The project warns"),
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
    let mut diagnostic = ara_diag::Diagnostic::error(ara_diag::Code(1), "Lamp is broken");
    diagnostic.location.record = Some("e_lamp".into());
    let row = |seq: u64, severity, diagnostic: ara_diag::Diagnostic| super::state::LogRow {
        seq,
        severity,
        code: "ARA0001".into(),
        message: diagnostic.message.clone(),
        origin: "engine".into(),
        at: 0,
        source: Some("levels/main.scene".into()),
        count: 1,
        diagnostic: std::sync::Arc::new(diagnostic),
    };
    let warning = ara_diag::Diagnostic::warning(ara_diag::Code(1), "Box is odd");
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
    for _ in 0..3000 {
        if rt.touch(|s| {
            s.editor.tick();
            super::asset_ops::poll_jobs(s);
            s.local.asset_jobs.is_empty()
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(content.join("art/notes.md").is_file(), "moved on disk");
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
fn lighting_recaptures_probes_and_applies_look_presets() {
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
    press(&mut rt, "lighting/probe/recapture");
    rt.update(|s| {
        s.poll_logs();
    });
    // Queued for the renderer, or (no renderer here) refused with its reason.
    let s = rt.state();
    let queued = super::lighting::capturing(s);
    let refused = s
        .logs
        .iter()
        .any(|r| r.severity == ara_diag::Severity::Error);
    assert!(queued || refused, "the recapture ran or said why not");
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
    assert!(
        rt.find("cmd/layout.load/Wide").is_some(),
        "Window lists saved layouts"
    );
    press(&mut rt, "menu/Window");
    // Open Recent lists the project's scenes.
    press(&mut rt, "menu/File");
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
    for _ in 0..3000 {
        if rt.touch(|s| {
            s.editor.tick();
            super::asset_ops::poll_jobs(s);
            s.local.asset_jobs.is_empty()
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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
    for light in [false, true] {
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
            let name = format!("{}-{panel}.png", if light { "light" } else { "dark" });
            std::fs::write(
                out.join(name),
                ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
            )
            .unwrap();
        }
        // A hovered Hierarchy row, its eye and lock offered.
        let lamp = rt.find("row/e_lamp").unwrap();
        rt.pointer_move(lamp.center().x + 40.0, lamp.center().y);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!(
            "{}-hierarchy-hover.png",
            if light { "light" } else { "dark" }
        );
        std::fs::write(
            out.join(name),
            ara_kit::capture::png(&rt.snap().unwrap()).unwrap(),
        )
        .unwrap();
        let row = rt.find("row/e_box").unwrap();
        rt.right_click(row.center().x, row.center().y);
        for _ in 0..30 {
            rt.frame(FRAME);
        }
        let name = format!("{}-context.png", if light { "light" } else { "dark" });
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
        let name = format!("{}-play.png", if light { "light" } else { "dark" });
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
            let name = format!("{}-tabdrag.png", if light { "light" } else { "dark" });
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
        let name = format!(
            "{}-profiler-compare.png",
            if light { "light" } else { "dark" }
        );
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
        let name = format!("{}-floating.png", if light { "light" } else { "dark" });
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
        let name = format!("{}-create.png", if light { "light" } else { "dark" });
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
        let name = format!("{}-settings.png", if light { "light" } else { "dark" });
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
        let name = format!("{}-palette.png", if light { "light" } else { "dark" });
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
        let name = format!("{}-add-component.png", if light { "light" } else { "dark" });
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
        let name = format!(
            "{}-asset-inspector.png",
            if light { "light" } else { "dark" }
        );
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
        allocated_bytes: 0,
        self_allocated_bytes: 0,
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
    rt.right_click(label.left - 120.0, label.center().y);
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
    let showcase = crate::asset_tools::node_preview::SHOWCASE.to_owned();
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
