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
        for panel in [
            "assets", "console", "problems", "agents", "history", "lighting", "profiler",
        ] {
            rt.update(|s| {
                super::commands::run(
                    s,
                    &format!("window.{panel}"),
                    &crate::shell::commands::Target::None,
                )
            });
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
            crate::shell::animation_panel::edit(&mut s.editor, &doc, "state").unwrap();
        }
        doc
    });
    rt.touch(|s| super::graph::sync(s, "animation"));
    let nodes = super::graph::node_ids(rt.state());
    assert_eq!(nodes.len(), 2, "{doc}: {nodes:?}");
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
            crate::shell::animation_panel::edit(&mut s.editor, &doc, kind).unwrap();
        }
        doc
    });
    rt.update(|s| {
        super::commands::run(s, "window.animation", &crate::shell::commands::Target::None)
    });
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
    rt.update(|s| {
        super::commands::run(s, "window.material", &crate::shell::commands::Target::None)
    });
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
    rt.update(|s| {
        super::commands::run(s, "window.material", &crate::shell::commands::Target::None)
    });
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
    rt.update(|s| {
        super::commands::run(s, "window.material", &crate::shell::commands::Target::None)
    });
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
    rt.update(|s| {
        super::commands::run(s, "window.material", &crate::shell::commands::Target::None)
    });
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
    rt.update(|s| super::commands::run(s, "window.vfx", &crate::shell::commands::Target::None));
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
