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

/// Writes the studio's chrome (the Scene hole left empty) in both themes, with each bottom
/// panel, to `target/studio-captures/` for design review: `cargo test -p ara-editor --lib
/// studio::tests::captures -- --ignored`.
#[test]
#[ignore = "writes review captures under target/"]
fn captures() {
    let mut rt = open();
    // The Project view over a real scan.
    rt.update(|s| {
        let store = s.editor.host.store();
        s.assets = Some(crate::shell::assets::Browser::new(
            s.editor.host.root(),
            store.schema(),
            store.workspace().reference_root(),
        ));
    });
    for _ in 0..200 {
        if rt.touch(|s| {
            s.assets
                .as_mut()
                .is_some_and(crate::shell::assets::Browser::poll)
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    rt.update(|s| {
        s.act(
            "editor.workspace",
            json!({"folder": ara_doc::AUTHORING_ROOT}),
        )
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
        for panel in ["assets", "console", "problems", "agents", "history"] {
            rt.update(|s| s.act("editor.layout.show", json!({"panel": panel})));
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
        rt.update(|s| {
            super::commands::run(s, "edit.preferences", &crate::shell::commands::Target::None)
        });
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
        rt.update(|s| {
            super::commands::run(s, "edit.palette", &crate::shell::commands::Target::None)
        });
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
    rt.update(|s| {
        super::commands::run(s, "edit.preferences", &crate::shell::commands::Target::None)
    });
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
    rt.update(|s| {
        super::commands::run(
            s,
            "layout.level_design",
            &crate::shell::commands::Target::None,
        )
    });
    settle(&mut rt);
    assert_ne!(
        format!("{:?}", rt.state().editor.state.layout),
        before,
        "the preset applies"
    );
    rt.update(|s| super::commands::run(s, "window.history", &crate::shell::commands::Target::None));
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
