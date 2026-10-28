#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
use ara_editor::{
    Editor,
    shell::Shell,
    world::{EditWorldView, gesture::transform_value},
};
use ara_proto::Endpoint;
use ara_ui::input::InputEvent;
use ara_ui_text::{Clipboard, EditEvent};
use glam::Vec3;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ara-editor-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::create_dir_all(root.join("settings")).unwrap();
        std::fs::write(
            root.join("settings/ara.imports"),
            ara_asset::ProjectImportSettings::default()
                .canonical_text()
                .unwrap(),
        )
        .unwrap();
        write(
            &root.join("ara.project.json"),
            json!({"name":"Editor test","layoutVersion":ara_host::project_layout::VERSION,"main":"levels/main.scene"}),
        );
        write(
            &root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
            json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{"e_crate001":{"name":"Salvage crate","Transform":{"position":[0,0.6,0]},"MeshRenderer":{}},"e_child001":{"name":"Child","parent":"e_crate001","Transform":{"position":[1,0,0]}}}}),
        );
        Self(root)
    }
    fn open(&self) -> Editor {
        Editor::open(&self.0).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ara-editor-test-")
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn write(path: &Path, value: Value) {
    std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap();
}
#[derive(Default)]
struct MemoryClipboard(String);
impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}
fn x(editor: &Editor) -> f32 {
    transform_value(
        &editor.view().unwrap().entities()["e_crate001"].record,
        "position",
    )
    .x
}

#[test]
fn only_scene_prefab_and_import_edits_reach_the_scene_through_its_views() {
    let project = Project::new();
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/other.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0009","records":{}}),
    );
    let mut editor = project.open();
    let scene = editor.state.doc.clone();
    let revision = editor.host.store().revision();
    editor
        .apply(
            vec![json!({"cmd":"transform.set","args":{"entities":["e_crate001"],"position":[4,0.6,0]}})],
            "Move",
            None,
        )
        .unwrap();
    assert!(ara_editor::world::scene_only(
        editor.host.store(),
        revision,
        &scene
    ));
    // The project's settings (like a material) are not in a scene view.
    let revision = editor.host.store().revision();
    let settings = editor
        .host
        .store()
        .workspace()
        .docs()
        .find(|(_, d)| d.kind == "project")
        .map(|(_, d)| d.id.to_string())
        .unwrap();
    editor
        .apply_to(
            &settings,
            vec![json!({"cmd":"project.setMain","args":{"scene":"levels/other.scene"}})],
            "Main scene",
        )
        .unwrap();
    assert!(!ara_editor::world::scene_only(
        editor.host.store(),
        revision,
        &scene
    ));
}

#[test]
fn native_hierarchy_inspector_gesture_and_author_undo_share_the_authority() {
    let project = Project::new();
    let mut editor = project.open();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let mut clipboard = MemoryClipboard::default();
    shell.click_entity("e_crate001", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(editor.state.selection, ["e_crate001"]);
    assert!(editor.host.store().history().is_empty());
    shell
        .click("edit-Transform-position-0", &mut clipboard)
        .unwrap();
    shell
        .event(InputEvent::Edit(EditEvent::SelectAll), &mut clipboard)
        .unwrap();
    shell
        .event(
            InputEvent::Edit(EditEvent::Insert("1.5m".into())),
            &mut clipboard,
        )
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.commit_fields(&mut editor).unwrap();
    assert_eq!(x(&editor), 1.5);
    editor.begin_gesture().unwrap();
    for value in [0.2, 0.4, 1.0] {
        editor
            .gesture
            .as_mut()
            .unwrap()
            .update(Vec3::new(value, 0., 0.));
    }
    editor.finish_gesture(false).unwrap();
    assert_eq!(x(&editor), 2.5);
    assert_eq!(editor.host.store().history().len(), 2);
    editor
        .action("editor.select", json!({"entities":["e_child001"]}))
        .unwrap();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(x(&editor), 1.5);
    editor.begin_gesture().unwrap();
    editor.gesture.as_mut().unwrap().update(Vec3::splat(50.));
    let revision = editor.host.store().revision();
    editor.finish_gesture(true).unwrap();
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(x(&editor), 1.5);
    editor.host.flush();
    drop(shell);
    drop(editor);
    assert_eq!(x(&project.open()), 1.5);
}

#[test]
fn mcp_agent_edits_apply_at_once_with_attribution_and_turn_undo() {
    let project = Project::new();
    let mut editor = project.open();
    let mut mcp = ara_mcp::Adapter::new();
    let args = json!({"action":"begin","client":"review-test","author":"agent:test"});
    let session = mcp
        .call_tool(
            &mut editor,
            "ara_session",
            args.as_object().unwrap().clone(),
        )
        .unwrap()["structuredContent"]["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let args = json!({"session":session,"doc":"sc_test0001","why":"Move salvage out of the doorway","commands":[{"cmd":"transform.set","args":{"entities":["e_crate001"],"position":[4,0.6,0]}}]});
    let result = mcp
        .call_tool(&mut editor, "ara_apply", args.as_object().unwrap().clone())
        .unwrap();
    assert_ne!(result["isError"], true, "{result}");
    let txn = result["structuredContent"]["txn"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(x(&editor), 4.);
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    editor
        .action("editor.focus", json!({"panel":"agents"}))
        .unwrap();
    // The window observes agent history at its frame boundary.
    editor.tick();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    assert!(shell.ui.find(&format!("turn-{txn}")).is_some());
    assert_eq!(
        shell.fields["Transform.position"].provenance,
        ara_editor::inspector::Provenance::AgentChanged
    );
    let mut clipboard = MemoryClipboard::default();
    shell
        .click(&format!("turn-{txn}-undo"), &mut clipboard)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(x(&editor), 0.);
    // Editor actions are exposed through the existing fixed MCP tool set.
    let args = json!({"action":"editor.theme","session":editor.session,"light":true});
    let result = mcp
        .call_tool(
            &mut editor,
            "ara_session",
            args.as_object().unwrap().clone(),
        )
        .unwrap();
    assert_ne!(result["isError"], true, "{result}");
    assert!(editor.state.light);
}

#[test]
fn agent_batches_apply_immediately_with_attribution_and_undo() {
    let project = Project::new();
    let mut editor = project.open();
    let session = Endpoint::call(
        &mut editor,
        "session.begin",
        json!({"client":"immediate","author":"agent:immediate"}),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let result = Endpoint::call(&mut editor,"cmd.apply",json!({"session":session,"doc":"sc_test0001","why":"Move now","commands":[{"cmd":"transform.set","args":{"entities":["e_crate001"],"position":[7,0.6,0]}}]})).unwrap();
    assert_eq!(x(&editor), 7.);
    assert_eq!(
        editor.host.store().history().last().unwrap().author,
        ara_ops::Author::Agent("immediate".into())
    );
    editor
        .action("review.revert", json!({"txn":result["txn"]}))
        .unwrap();
    assert_eq!(x(&editor), 0.);
}

#[test]
fn gizmos_use_visible_local_world_axes_and_rotation_rings() {
    use ara_editor::{shell::overlay, state::Tool, world::gesture::Gesture};
    let project = Project::new();
    let mut editor = project.open();
    editor.apply(vec![json!({"cmd":"transform.set","args":{"entities":["e_crate001"],"rotation":[0,0,90]}})],"Rotate fixture",None).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    let view = editor.view().unwrap();
    for (local, expected) in [
        (false, Vec3::new(1., 2., 1.)),
        (true, Vec3::new(2., 1., 1.)),
    ] {
        let mut state = editor.state.clone();
        state.tool = Tool::Scale;
        state.local = local;
        let mut gesture =
            Gesture::begin(view.entities(), &state, editor.host.store().revision()).unwrap();
        gesture.update(Vec3::X);
        let scale = gesture.current["e_crate001"]
            .to_scale_rotation_translation()
            .0;
        assert!(scale.abs_diff_eq(expected, 0.001), "{local}: {scale:?}");
        gesture.update(Vec3::ZERO);
        assert!(gesture.commands().is_empty());
    }
    let mut state = editor.state.clone();
    state.tool = Tool::Rotate;
    state.local = true;
    let rect = ara_ui::paint::Box2 {
        x: 0.,
        y: 0.,
        w: 800.,
        h: 600.,
    };
    let handles = overlay::handles(&view, &state, rect).unwrap();
    for axis in 0..3 {
        let ring = (0..48)
            .find_map(|n| {
                let [a, b] = handles.ring_segment(axis, n)?;
                let p = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                (overlay::hit(&handles, p) == Some(overlay::Handle::Axis(axis))).then_some(p)
            })
            .expect("visible ring must be pickable");
        let point =
            overlay::plane_point(&state.camera, rect, ring, handles.pivot, handles.axes[axis])
                .unwrap();
        assert!((point - handles.pivot).dot(handles.axes[axis]).abs() < 0.001);
    }
}

#[test]
fn every_gizmo_handle_is_grabbable_where_it_is_drawn() {
    use ara_editor::{shell::overlay, state::Tool};
    use glam::Vec2;
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    let view = editor.view().unwrap();
    let rect = ara_ui::paint::Box2 {
        x: 0.,
        y: 0.,
        w: 800.,
        h: 600.,
    };
    let mut state = editor.state.clone();
    state.tool = Tool::Translate;
    let handles = overlay::handles(&view, &state, rect).unwrap();
    let o = Vec2::from(handles.origin);
    assert_eq!(
        overlay::hit(&handles, o.into()),
        Some(overlay::Handle::Centre)
    );
    let mut planes = 0;
    for axis in 0..3 {
        let end = Vec2::from(handles.ends[axis]);
        if o.distance(end) >= 30. {
            // Along the shaft and at the cone, 7 px off the line: a 16 px target.
            for t in [0.6, 0.95] {
                let off = (end - o).perp().normalize() * 7.;
                let p = o + (end - o) * t + off;
                assert_eq!(
                    overlay::hit(&handles, p.into()),
                    Some(overlay::Handle::Axis(axis)),
                    "axis {axis} at {t}"
                );
            }
        }
        // A plane square grabs anywhere inside what is drawn.
        if let Some(quad) = handles.plane_quad(axis) {
            planes += 1;
            let centre = quad.iter().map(|c| Vec2::from(*c)).sum::<Vec2>() / 4.;
            assert_eq!(
                overlay::hit(&handles, centre.into()),
                Some(overlay::Handle::Plane(axis)),
                "plane {axis}"
            );
        }
    }
    assert!(
        planes >= 2,
        "the default view shows at least two plane squares"
    );
}

#[test]
fn universal_handles_pick_the_operation_and_keep_transform_commands_typed() {
    use ara_editor::{
        shell::overlay::{self, Handle},
        state::Tool,
        world::gesture::Gesture,
    };
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    editor
        .action("editor.tool", json!({"tool":"universal"}))
        .unwrap();
    let view = editor.view().unwrap();
    let rect = ara_ui::paint::Box2 {
        x: 0.,
        y: 0.,
        w: 800.,
        h: 600.,
    };
    let handles = overlay::handles(&view, &editor.state, rect).unwrap();
    assert_eq!(overlay::hit(&handles, handles.origin), Some(Handle::Centre));
    let scale = handles.for_tool(Tool::Scale);
    for axis in 0..3 {
        assert_eq!(
            overlay::hit(&handles, scale.ends[axis]),
            Some(Handle::ScaleAxis(axis))
        );
        let rotate = handles.for_tool(Tool::Rotate);
        assert!(
            (0..48).any(|n| {
                rotate.ring_segment(axis, n).is_some_and(|[a, b]| {
                    let p = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                    overlay::hit(&handles, p) == Some(Handle::RotateAxis(axis))
                })
            }),
            "rotation ring {axis} has a reachable target"
        );
        assert_eq!(Handle::RotateAxis(axis).tool(Tool::Universal), Tool::Rotate);
        assert_eq!(Handle::ScaleAxis(axis).tool(Tool::Universal), Tool::Scale);
    }
    for (tool, delta, property) in [
        (Tool::Translate, Vec3::X, "position"),
        (Tool::Rotate, Vec3::Y * 15., "rotation"),
        (Tool::Scale, Vec3::splat(0.25), "scale"),
    ] {
        let mut gesture = Gesture::begin_tool(
            view.entities(),
            &editor.state,
            editor.host.store().revision(),
            tool,
        )
        .unwrap();
        gesture.update(delta);
        let commands = gesture.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["cmd"], "transform.set");
        assert!(commands[0]["args"][property].is_array());
        gesture.update(Vec3::ZERO);
        assert!(
            gesture.commands().is_empty(),
            "cancel returns to the initial pose"
        );
        assert_eq!(editor.state.tool, Tool::Universal);
    }
}

#[test]
fn layout_persistence_units_and_multi_edit_are_validated() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action("editor.layout", json!({"panel":"agents","area":"right"}))
        .unwrap();
    assert_eq!(
        ara_editor::inspector::parse_number("90deg", Some("deg")).unwrap(),
        90.
    );
    assert_eq!(
        ara_editor::inspector::parse_number("150cm", Some("m")).unwrap(),
        1.5
    );
    assert!(ara_editor::inspector::parse_number("3deg", Some("m")).is_err());
    editor
        .action(
            "editor.select",
            json!({"entities":["e_crate001","e_child001"]}),
        )
        .unwrap();
    editor
        .action("editor.focus", json!({"panel":"inspector"}))
        .unwrap();
    let layout = editor.state.layout.clone();
    let shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    assert!(shell.fields["Transform.position"].mixed);
    editor
        .apply(
            ara_editor::inspector::edit_commands(
                &editor.state.selection,
                "Transform.position.x",
                &json!(2.),
            ),
            "Multi edit",
            None,
        )
        .unwrap();
    assert_eq!(x(&editor), 2.);
    drop(shell);
    drop(editor);
    let mut editor = project.open();
    assert_eq!(editor.state.layout, layout);
    editor.action("editor.layout.reset", json!({})).unwrap();
    assert_eq!(editor.state.layout, ara_editor::state::default_layout());
}

#[test]
fn agent_presence_does_not_take_the_human_camera_and_agent_deletes_apply_with_turn_undo() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action("editor.select", json!({"entities":["e_child001"]}))
        .unwrap();
    let human = editor.state.camera;
    let agent = Endpoint::call(
        &mut editor,
        "session.begin",
        json!({"client":"presence","author":"agent:presence"}),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    Endpoint::call(
        &mut editor,
        "editor.select",
        json!({"session":agent,"entities":["e_crate001"]}),
    )
    .unwrap();
    Endpoint::call(
        &mut editor,
        "editor.camera",
        json!({"session":agent,"target":[3,2,1],"yaw":1.,"pitch":0.2,"distance":6.}),
    )
    .unwrap();
    assert_eq!(editor.state.camera, human);
    assert_eq!(editor.state.selection, ["e_child001"]);
    assert_eq!(editor.state.presence[&agent].selection, ["e_crate001"]);
    // The human is told what the agent looks at, with View/Follow; nothing moves.
    assert!(editor.activity.notices().iter().any(|n| {
        n.text == "Presence is viewing main.scene · Salvage crate"
            && n.kind
                == ara_editor::activity::NoticeKind::AgentView {
                    agent: agent.clone(),
                }
    }));
    editor
        .action("editor.follow", json!({"agent":agent}))
        .unwrap();
    assert_eq!(editor.state.camera.distance, 6.);
    editor.action("editor.follow", json!({})).unwrap();
    let result=Endpoint::call(&mut editor,"cmd.apply",json!({"session":agent,"doc":"sc_test0001","why":"Remove obsolete crate","commands":[{"cmd":"entity.delete","args":{"entities":["e_crate001"]}}]})).unwrap();
    // D24: the delete just applies (no reviewable ghost); the turn can be undone.
    let view = editor.view().unwrap();
    assert!(view.entities().is_empty());
    let scene = view
        .scene(
            &project.0,
            &editor.state,
            None,
            &mut ara_editor::world::ModelCache::new(&project.0).unwrap(),
        )
        .unwrap();
    assert!(
        !scene.meshes.iter().any(|m| m.name.contains("deleted")),
        "{:?}",
        scene.meshes.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
    editor.tick();
    assert!(!editor.agent_tints().is_empty(), "the agent's turn glows");
    editor
        .action("review.revert", json!({"txn":result["txn"]}))
        .unwrap();
    assert_eq!(editor.view().unwrap().entities().len(), 2);
    editor.tick();
    assert!(
        editor.agent_tints().is_empty(),
        "an undone turn stops glowing"
    );
}

#[test]
fn duplicate_reparent_and_reset_are_one_command_each_with_undo() {
    let project = Project::new();
    let mut editor = project.open();
    let result = editor
        .apply(
            vec![json!({"cmd":"editor.entity.duplicate","args":{"entity":"e_crate001"}})],
            "Duplicate crate",
            None,
        )
        .unwrap();
    assert_eq!(editor.view().unwrap().entities().len(), 4);
    let id = result["outputs"][0]["id"].as_str().unwrap();
    assert_eq!(
        editor.view().unwrap().entities()[id].record["name"],
        "Salvage crate copy"
    );
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(editor.view().unwrap().entities().len(), 2);
    editor.apply(vec![json!({"cmd":"entity.reparent","args":{"entity":"e_child001"}}),json!({"cmd":"property.set","args":{"entity":"e_crate001","path":"Transform.position.x","value":3}})],"Reparent and position",None).unwrap();
    editor.apply(vec![json!({"cmd":"editor.property.reset","args":{"entity":"e_crate001","path":"Transform.position"}})],"Reset",None).unwrap();
    assert_eq!(x(&editor), 0.);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(x(&editor), 3.);
}

#[test]
#[ignore = "100k hierarchy scale measurement; run explicitly on the workstation"]
fn hierarchy_100k_keeps_a_bounded_row_pool_and_reaches_the_last_entity() {
    let project = Project::new();
    let records: serde_json::Map<String, Value> = (0..100_000)
        .map(|i| {
            (
                format!("e_{i:08}"),
                json!({"name":format!("Entity {i:08}"),"Transform":{}}),
            )
        })
        .collect();
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":records}),
    );
    let mut editor = project.open();
    let start = std::time::Instant::now();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let build_ms = start.elapsed().as_secs_f64() * 1000.;
    let node_count = shell.ui.nodes.len();
    assert!(
        node_count < 1_000,
        "hierarchy allocated {node_count} retained nodes"
    );
    let i = shell.ui.find("hierarchy-rows").unwrap();
    let rect = shell.ui.nodes[i].rect;
    shell.pointer = [rect.x + 20., rect.y + 20.];
    let start = std::time::Instant::now();
    assert!(shell.scroll_hierarchy(2_600_000.).unwrap());
    shell.frame().unwrap();
    let scroll_ms = start.elapsed().as_secs_f64() * 1000.;
    assert_eq!(shell.ui.nodes.len(), node_count);
    let work = shell.frame().unwrap();
    assert_eq!(
        work.paint_nodes, 0,
        "idle hierarchy must retain its display list"
    );
    shell
        .click_entity("e_00099999", &mut MemoryClipboard::default())
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(editor.state.selection, ["e_00099999"]);
    eprintln!(
        "100k hierarchy: build {build_ms:.3} ms; scroll {scroll_ms:.3} ms; {node_count} retained nodes"
    );
    // Tests never write tracked files: the checked-in measurement changes only on
    // request (ARA_EDITOR_UPDATE_EVIDENCE=1).
    let output = if std::env::var_os("ARA_EDITOR_UPDATE_EVIDENCE").is_some_and(|v| v == "1") {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("evidence/hierarchy-100k.json")
    } else {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hierarchy-100k.json")
    };
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    write(
        &output,
        json!({"entities":100000,"retainedNodes":node_count,"buildMs":build_ms,"scrollMs":scroll_ms,"idlePaintNodes":work.paint_nodes,"lastEntitySelected":true}),
    );
}

#[test]
fn numeric_scrubbing_is_one_undoable_batch_and_escape_is_nonmutating() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action(
            "editor.select",
            json!({"entities":["e_crate001","e_child001"]}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let chip = shell.ui.find("scrub-Transform-position-0").unwrap();
    let r = shell.ui.nodes[chip].rect;
    shell.pointer = [r.x + r.w * 0.5, r.y + r.h * 0.5];
    let count = editor.host.store().history().len();
    assert!(shell.begin_scrub(&mut editor).unwrap());
    shell.pointer[0] += 25.;
    assert!(shell.move_scrub().unwrap());
    assert_eq!(x(&editor), 0.);
    assert!(shell.finish_scrub(&mut editor, false).unwrap());
    assert_eq!(x(&editor), 0.25);
    assert_eq!(
        transform_value(
            &editor.view().unwrap().entities()["e_child001"].record,
            "position"
        )
        .x,
        1.25
    );
    assert_eq!(editor.host.store().history().len(), count + 1);
    let i = shell.ui.find("scrub-Transform-position-0").unwrap();
    let r = shell.ui.nodes[i].rect;
    shell.pointer = [r.x + 10., r.y + 10.];
    shell.begin_scrub(&mut editor).unwrap();
    shell.pointer[0] += 100.;
    shell.move_scrub().unwrap();
    shell.finish_scrub(&mut editor, true).unwrap();
    assert_eq!(x(&editor), 0.25);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(x(&editor), 0.);
    editor
        .action(
            "editor.preferences",
            json!({"reduceMotion":true,"quality":false}),
        )
        .unwrap();
    assert!(editor.state.reduce_motion);
    assert!(!editor.state.quality);
}

#[test]
fn import_settings_are_typed_attributed_and_undoable() {
    let project = Project::new();
    std::fs::write(
        project.0.join(ara_doc::AUTHORING_ROOT).join("paint.png"),
        b"invalid source; settings test does not run an importer",
    )
    .unwrap();
    let mut editor = project.open();
    let result = editor
        .action("editor.asset.import", json!({"path":"paint.png"}))
        .unwrap();
    editor
        .action("jobs.cancel", json!({"job":result["job"]}))
        .unwrap();
    let sidecar = editor
        .host
        .store()
        .workspace()
        .storage_path("paint.png.import.json");
    let doc = editor
        .host
        .store()
        .workspace()
        .docs()
        .find_map(|(path, doc)| (path == sidecar).then(|| doc.id.to_string()))
        .unwrap();
    assert_eq!(
        editor.host.store().workspace().index().path_of(&doc),
        Some("paint.png")
    );
    assert_eq!(
        editor.host.store().workspace().path_of(&doc),
        Some(sidecar.as_str())
    );
    editor.apply_to(&doc,vec![json!({"cmd":"import.configure","args":{"source":"paint.png","settings":{"usage":"normal","mipmaps":false},"preset":"balanced"}})],"Use a normal map").unwrap();
    assert_eq!(
        editor
            .host
            .store()
            .workspace()
            .doc(&doc)
            .unwrap()
            .record("i_settings")
            .unwrap()["settings"]["usage"],
        "normal"
    );
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(
        editor
            .host
            .store()
            .workspace()
            .doc(&doc)
            .unwrap()
            .record("i_settings")
            .unwrap()["settings"],
        json!({})
    );
    assert!(editor.apply_to(&doc,vec![json!({"cmd":"import.configure","args":{"source":"paint.png","settings":{"unknown":true},"preset":"fast"}})],"Invalid setting").is_err());
    assert!(
        editor
            .action("editor.asset.import", json!({"path":"../escape.png"}))
            .is_err()
    );
}

#[test]
fn material_canvas_create_connect_and_keyboard_undo() {
    let project = Project::new();
    let mut editor = project.open();
    // Tool pages open floating; docked into the view, the main shell shows it.
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"material","slot":"view"}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let mut clipboard = MemoryClipboard::default();
    shell.click("material-empty-new", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    let doc = editor.state.resource.clone().unwrap();
    let before =
        ara_graph::Graph::from_document(editor.host.store().workspace().doc(&doc).unwrap())
            .unwrap();
    shell.click("graph-add", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(
        shell.input.focused(),
        Some(ara_ui::source::stable_id("search-nodes"))
    );
    let rect = shell.ui.nodes[shell.ui.find("node-palette").unwrap()].rect;
    assert!((rect.x + rect.w * 0.5 - 720.).abs() < 0.5);
    shell.key(&mut editor, 41, true, &mut clipboard).unwrap();
    assert!(!editor.state.canvas_add);
    shell.click("graph-add", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    editor
        .action(
            "editor.search",
            json!({"panel":"nodes","query":"material.add"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.click("node-add-0", &mut clipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    let added = ara_graph::Graph::from_document(editor.host.store().workspace().doc(&doc).unwrap())
        .unwrap();
    assert_eq!(added.nodes.len(), before.nodes.len() + 3);
    let node = added
        .nodes
        .iter()
        .find(|(_, n)| n.kind == "material.add")
        .unwrap()
        .0
        .clone();
    let source = added
        .nodes
        .iter()
        .find(|(_, n)| n.payload["name"] == "roughness")
        .unwrap()
        .0
        .clone();
    let to = ara_graph::Endpoint {
        node: node.clone(),
        port: "in0".into(),
    };
    let from = ara_graph::Endpoint {
        node: source.clone(),
        port: "out".into(),
    };
    editor
        .action("editor.canvas", json!({"zoom":0.45,"pan":[20,20]}))
        .unwrap();
    shell.sync_canvas(&editor);
    shell.frame().unwrap();
    // The canvas splices in under its panel; decorations still move as a block, so
    // clip pushes and pops stay balanced and in order.
    let mut depth = 0i32;
    for draw in &shell.ui.display.commands {
        match draw {
            ara_ui::paint::Draw::PushClip { .. } | ara_ui::paint::Draw::PushRoundedClip { .. } => {
                depth += 1
            }
            ara_ui::paint::Draw::PopClip => depth -= 1,
            _ => {}
        }
        assert!(depth >= 0, "a clip pop precedes its push");
    }
    assert_eq!(depth, 0, "unbalanced clips");
    let a = shell.graph_port(&from).unwrap();
    let b = shell.graph_port(&to).unwrap();
    shell.pointer = a;
    shell
        .graph_event(
            &mut editor,
            &ara_platform::Event::MouseButton {
                button: 1,
                pressed: true,
            },
        )
        .unwrap();
    shell
        .graph_event(
            &mut editor,
            &ara_platform::Event::MouseMotion {
                position: b,
                delta: [b[0] - a[0], b[1] - a[1]],
            },
        )
        .unwrap();
    shell
        .graph_event(
            &mut editor,
            &ara_platform::Event::MouseButton {
                button: 1,
                pressed: false,
            },
        )
        .unwrap();
    let connected =
        ara_graph::Graph::from_document(editor.host.store().workspace().doc(&doc).unwrap())
            .unwrap();
    assert_eq!(
        connected
            .edges
            .values()
            .find(|e| e.to == to)
            .unwrap()
            .from
            .node,
        source
    );
    shell.key(&mut editor, 224, true, &mut clipboard).unwrap();
    shell.key(&mut editor, 29, true, &mut clipboard).unwrap();
    shell.key(&mut editor, 224, false, &mut clipboard).unwrap();
    let undone =
        ara_graph::Graph::from_document(editor.host.store().workspace().doc(&doc).unwrap())
            .unwrap();
    assert_eq!(undone.edges, added.edges);
    let material = ara_material::Material::from_graph(&undone).unwrap();
    let source = ara_material::dsl::print(&material).unwrap();
    assert!(source.contains("add("));
}

#[test]
fn material_values_and_animation_layout_share_undo_and_survive_reopen() {
    let project = Project::new();
    let mut editor = project.open();
    let material = editor
        .action(
            "editor.resource.create",
            json!({"domain":"material","path":"paint.graph"}),
        )
        .unwrap()["doc"]
        .as_str()
        .unwrap()
        .to_owned();
    let graph =
        ara_graph::Graph::from_document(editor.host.store().workspace().doc(&material).unwrap())
            .unwrap();
    let node = graph
        .nodes
        .iter()
        .find(|(_, n)| n.payload["name"] == "roughness")
        .unwrap()
        .0
        .clone();
    editor
        .apply_to(
            &material,
            vec![json!({"cmd":"editor.material.value.set","args":{"node":node,"value":[0.82]}})],
            "Set roughness",
        )
        .unwrap();
    let changed =
        ara_graph::Graph::from_document(editor.host.store().workspace().doc(&material).unwrap())
            .unwrap();
    let ast = ara_material::Material::from_graph(&changed).unwrap();
    assert_eq!(
        ast.parameters[&node].default,
        ara_material::ParameterValue::Numeric(vec![0.82])
    );
    assert!(ara_material::dsl::print(&ast).unwrap().contains("0.82"));
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(
        ara_graph::Graph::from_document(editor.host.store().workspace().doc(&material).unwrap())
            .unwrap(),
        graph
    );
    editor
        .action("editor.workspace", json!({"resource":material}))
        .unwrap();
    editor
        .action("editor.canvas", json!({"selection":[node]}))
        .unwrap();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"material","slot":"view"}),
        )
        .unwrap();
    let mut invalid_shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    editor.apply_to(&material,vec![json!({"cmd":"graph.param.set","args":{"node":node,"name":"value","value":"invalid numeric default"}})],"Verify node diagnostics").unwrap();
    invalid_shell.rebuild(&mut editor).unwrap();
    // The Material tool's status strip names the first error.
    assert!(
        invalid_shell
            .ui
            .find("material-status-error-text")
            .is_some()
    );
    assert!(
        invalid_shell.layout_report()["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == node && n["error"].is_string())
    );
    editor.action("history.undo", json!({})).unwrap();
    invalid_shell.rebuild(&mut editor).unwrap();
    assert!(
        invalid_shell
            .ui
            .find("material-status-error-text")
            .is_none()
    );
    drop(invalid_shell);

    let animation = editor
        .action(
            "editor.resource.create",
            json!({"domain":"animation","path":"motion.animation"}),
        )
        .unwrap()["doc"]
        .as_str()
        .unwrap()
        .to_owned();
    editor
        .action("editor.workspace", json!({"resource":animation}))
        .unwrap();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"animation","slot":"view"}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let mut clipboard = MemoryClipboard::default();
    for control in [
        "animation-add-state",
        "animation-add-state",
        "animation-add-blend",
        "animation-add-transition",
    ] {
        shell.click(control, &mut clipboard).unwrap();
        shell.dispatch(&mut editor).unwrap();
    }
    let controller = editor
        .host
        .store()
        .workspace()
        .doc(&animation)
        .unwrap()
        .record("an_controller")
        .unwrap()["controller"]
        .clone();
    assert_eq!(controller["states"].as_object().unwrap().len(), 3);
    assert_eq!(controller["transitions"].as_array().unwrap().len(), 1);
    let mut invalid = controller.clone();
    let blend = invalid["nodes"]
        .as_object()
        .unwrap()
        .keys()
        .find(|key| key.starts_with("Locomotion"))
        .unwrap()
        .clone();
    invalid["nodes"][&blend]["samples"][0]["node"] = json!(blend);
    let before = editor.host.store().revision();
    assert!(editor.apply_to(&animation,vec![json!({"cmd":"editor.animation.controller.set","args":{"record":"an_controller","controller":invalid}})],"Invalid cycle").is_err());
    assert_eq!(editor.host.store().revision(), before);
    editor.action("editor.animation.layout",json!({"doc":animation,"editor":{"comments":{"note":{"text":"Locomotion","position":[20,30]}}}})).unwrap();
    let layout_path = format!(".ara/editor/animation-{animation}.graph");
    assert!(editor.host.store().workspace().find(&layout_path).is_some());
    editor.host.flush();
    assert!(project.0.join(&layout_path).is_file());
    assert!(
        !project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join(&layout_path)
            .exists()
    );
    editor.host.flush();
    drop(shell);
    drop(editor);
    let reopened = project.open();
    let restored: ara_anim::graph::ControllerData = serde_json::from_value(
        reopened
            .host
            .store()
            .workspace()
            .doc(&animation)
            .unwrap()
            .record("an_controller")
            .unwrap()["controller"]
            .clone(),
    )
    .unwrap();
    assert_eq!(
        restored,
        serde_json::from_value::<ara_anim::graph::ControllerData>(controller).unwrap()
    );
    let layout = reopened
        .host
        .store()
        .workspace()
        .find(&layout_path)
        .unwrap();
    let graph = ara_graph::Graph::from_document(
        reopened
            .host
            .store()
            .workspace()
            .doc(layout.as_str())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(graph.editor.comments["note"].text, "Locomotion");
}

/// Tests never touch the owner's profile (%APPDATA%\Ara, %LOCALAPPDATA%\Ara): cargo
/// gives every test and the editors it starts isolated directories under target/
/// (`.cargo/config.toml`), and without them the editor refuses to start.
#[test]
fn tests_resolve_isolated_profile_directories_under_target() {
    let config = ara_editor::settings::config_dir().expect("an isolated config directory");
    let crashes = std::env::var_os("ARA_CRASH_DIR")
        .map(std::path::PathBuf::from)
        .expect("ARA_CRASH_DIR");
    for dir in [&config, &crashes] {
        assert!(
            dir.components().any(|c| c.as_os_str() == "target"),
            "{dir:?} is not under target/"
        );
        for profile in [
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
        ] {
            if let Some(profile_dir) = std::env::var_os(profile) {
                assert!(
                    !dir.starts_with(&profile_dir),
                    "{dir:?} is inside {profile}"
                );
            }
        }
    }
}

/// The Scene quality is a user preference: it round-trips through the isolated
/// profile and never lands in the project. The project's default is
/// `rendering.quality` in settings/project.json, edited through the typed settings command.
#[test]
fn scene_quality_persists_as_a_user_preference_and_canonical_project_policy() {
    use ara_render_fidelity::presets::{QualityChoice, QualityPreset};
    let project = Project::new();
    let original_marker: Value =
        serde_json::from_str(&std::fs::read_to_string(project.0.join("ara.project.json")).unwrap())
            .unwrap();
    let profile = ara_editor::settings::config_dir()
        .unwrap()
        .join(format!("quality-roundtrip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    let preferences = profile.join("editor-settings.json");
    assert!(
        preferences.components().any(|c| c.as_os_str() == "target"),
        "{preferences:?}"
    );
    {
        let mut editor = Editor::open_with(&project.0, Some(preferences.clone())).unwrap();
        editor
            .action("editor.preferences", json!({"renderQuality":"epic"}))
            .unwrap();
        editor
            .action(
                "editor.settings.set",
                json!({"key":"rendering.quality","value":"medium"}),
            )
            .unwrap();
    }
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&preferences).unwrap()).unwrap();
    assert_eq!(saved["viewport.quality"], "epic");
    let policy: Value = serde_json::from_str(
        &std::fs::read_to_string(project.0.join(ara_host::project_policy::PREFERENCES)).unwrap(),
    )
    .unwrap();
    assert_eq!(policy["rendering"]["quality"], "medium");
    assert!(!policy.to_string().contains("viewport"), "{policy}");
    let marker: Value =
        serde_json::from_str(&std::fs::read_to_string(project.0.join("ara.project.json")).unwrap())
            .unwrap();
    assert_eq!(
        marker, original_marker,
        "Quality edits preserve project identity metadata"
    );
    assert!(marker.get("rendering").is_none());

    let mut editor = Editor::open_with(&project.0, Some(preferences.clone())).unwrap();
    assert_eq!(
        editor.state.render_quality,
        Some(QualityChoice::Preset(QualityPreset::Epic))
    );
    assert_eq!(
        editor.state.project_quality(),
        QualityChoice::Preset(QualityPreset::Medium)
    );
    assert!(
        editor
            .host
            .store()
            .workspace()
            .doc(ara_host::project_policy::PREFERENCES_ID)
            .is_some()
    );
    assert_eq!(
        ara_host::project_policy::policy(editor.host.store().workspace(), "rendering"),
        Some(&json!({"quality":"medium"}))
    );
    // Clearing the override falls back to the project's default.
    editor
        .action("editor.preferences", json!({"renderQuality":"project"}))
        .unwrap();
    assert_eq!(
        editor.state.render_choice(),
        QualityChoice::Preset(QualityPreset::Medium)
    );
    assert!(editor.state.quality);
    editor
        .action(
            "editor.settings.set",
            json!({"key":"rendering.quality","value":"low"}),
        )
        .unwrap();
    assert!(
        !editor.state.quality,
        "a Low project default is the raster view"
    );
    drop(editor);

    // Preferences from before presets: Raster becomes Low.
    std::fs::write(&preferences, r#"{"viewport.quality":"raster"}"#).unwrap();
    let editor = Editor::open_with(&project.0, Some(preferences.clone())).unwrap();
    assert_eq!(
        editor.state.render_quality,
        Some(QualityChoice::Preset(QualityPreset::Low))
    );
    drop(editor);
    std::fs::remove_dir_all(&profile).unwrap();
}

/// A scene that references a missing model file still opens: the entity draws a
/// placeholder and Problems names the path (ARA4506), once per path.
#[test]
fn missing_model_sources_open_with_placeholders_and_name_the_path() {
    use ara_editor::world::EditWorldView;
    let project = Project::new();
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{
            "e_hangar01":{"name":"Hangar","Transform":{},"MeshRenderer":{"model":"@assets/kit/hangar.gltf#as_kithangar"}},
            "e_crate001":{"name":"Salvage crate","Transform":{"position":[0,0.6,0]},"MeshRenderer":{}}}}),
    );
    let mut editor = project.open();
    let view = editor.view().unwrap();
    let scene = view
        .scene(
            &project.0,
            &editor.state,
            None,
            &mut ara_editor::world::ModelCache::new(&project.0).unwrap(),
        )
        .unwrap();
    assert!(
        scene.meshes.iter().any(|m| m.name == "Hangar"),
        "a placeholder"
    );
    editor.report_missing_sources(&view);
    editor.report_missing_sources(&view);
    let logs = editor
        .action("logs", json!({"codes": ["ARA4506"], "group": false}))
        .unwrap();
    assert_eq!(logs["total"], 1, "{logs}");
    let text = logs["text"].as_str().unwrap();
    assert!(
        text.contains("`assets/kit/hangar.gltf` is missing"),
        "{text}"
    );
}

/// The Scene view camera carries its vertical field of view: `editor.camera` reads
/// it, sets it within 10–120°, and looks through a scene Camera entity (pose and
/// FOV); `spec()` hands it to the renderer.
#[test]
fn scene_view_fov_is_read_set_clamped_and_aligned_to_a_scene_camera() {
    let project = Project::new();
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{
            "e_crate001":{"name":"Salvage crate","Transform":{"position":[0,0.6,0]},"MeshRenderer":{}},
            "e_rig00001":{"name":"Rig","Transform":{"position":[0,1,4]}},
            "e_shot0001":{"name":"Shot","parent":"e_rig00001","Transform":{"position":[2,0.5,2],"rotation":[-12,25,0]},"Camera":{"fov":46}}}}),
    );
    let mut editor = project.open();
    let read = editor.action("editor.camera", json!({})).unwrap();
    assert_eq!(
        read["camera"]["fovYDegrees"], 46.,
        "a scene opens through its gameplay camera"
    );
    editor
        .action("editor.camera", json!({"fovYDegrees": 200}))
        .unwrap();
    assert_eq!(editor.state.camera.fov_y_degrees, 120.);
    let set = editor
        .action("editor.camera", json!({"fovYDegrees": 54}))
        .unwrap();
    assert_eq!(set["camera"]["fovYDegrees"], 54.);
    let ara_render::scene::CameraSpec::LookAt { vertical_fov, .. } = editor.state.camera.spec()
    else {
        panic!("the Scene view is a perspective camera")
    };
    assert!((vertical_fov - 54f32.to_radians()).abs() < 1e-6);

    // Looking through the shot copies its world pose (under its parent) and FOV.
    editor
        .action("editor.camera", json!({"entity":"e_shot0001"}))
        .unwrap();
    let camera = editor.state.camera;
    assert_eq!(camera.fov_y_degrees, 46.);
    assert!(
        camera.position().distance(Vec3::new(2., 1.5, 6.)) < 1e-4,
        "{:?}",
        camera.position()
    );
    let rotation = glam::Quat::from_euler(
        glam::EulerRot::XYZ,
        (-12f32).to_radians(),
        25f32.to_radians(),
        0.,
    );
    let forward = (camera.target - camera.position()).normalize();
    assert!(forward.dot(rotation * Vec3::NEG_Z) > 0.99999, "{forward:?}");
    let error = editor
        .action("editor.camera", json!({"entity":"e_crate001"}))
        .unwrap_err();
    assert!(error.to_string().contains("no Camera component"), "{error}");

    // Cameras saved before the field existed get the default.
    let saved: ara_editor::state::Camera =
        serde_json::from_value(json!({"target":[0,0,0],"yaw":0,"pitch":0.2,"distance":5})).unwrap();
    assert_eq!(saved.fov_y_degrees, 50.);
}

/// A scene opens with its last camera (a user preference beside
/// editor-settings.json, FOV included), else its gameplay camera's view (a follow
/// rig pivots on what it aims at), else its content without terrain or ground; F
/// and Frame All skip terrain and ground planes unless they are selected.
#[test]
fn scenes_open_with_their_saved_or_gameplay_camera_and_framing_skips_terrain() {
    let project = Project::new();
    write(
        &project.0.join("ara.project.json"),
        json!({"name":"Editor test","layoutVersion":ara_host::project_layout::VERSION,"main":"levels/main.scene"}),
    );
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{
            "e_crate001":{"name":"Salvage crate","Transform":{"position":[2,0.5,-3]},"MeshRenderer":{}},
            "e_ground01":{"name":"Ground","Transform":{},"MeshRenderer":{"shape":"plane","size":[400,1,400]}},
            "e_chunk001":{"name":"Terrain chunk","Transform":{"position":[300,0,300],"scale":[96,10,96]},"TerrainChunk":{"x":3,"z":3}}}}),
    );
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/rig.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0002","records":{
            "e_player01":{"name":"Player","Transform":{"position":[10,0,4]},"MeshRenderer":{}},
            "e_camera01":{"name":"Follow camera","Transform":{"position":[0,9,0]},"Camera":{"fov":46},
                "FollowCamera":{"target":"e_player01","distance":4,"height":1.5,"yaw":30,"pitch":15}}}}),
    );
    // A static wall 2 m behind the player (a sensor before it does not count).
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/wall.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0003","records":{
            "e_player01":{"name":"Player","Transform":{},"MeshRenderer":{}},
            "e_wall0001":{"name":"Wall","Transform":{"position":[0,1,-2]},"Collider":{"halfExtents":[5,3,0.1]},"RigidBody":{}},
            "e_sensor01":{"name":"Trigger","Transform":{"position":[0,1,-1]},"Collider":{"halfExtents":[5,3,0.1],"sensor":true},"RigidBody":{}},
            "e_camera01":{"name":"Follow camera","Transform":{},"Camera":{},
                "FollowCamera":{"target":"e_player01","distance":4,"height":1.5,"pitch":0}}}}),
    );
    let crate_centre = Vec3::new(2., 0.5, -3.);
    {
        let mut editor = project.open();
        // No gameplay camera: the crate, not the 400 m ground or the far terrain.
        let camera = editor.state.camera;
        assert!(camera.target.distance(crate_centre) < 1e-4, "{camera:?}");
        assert!(camera.distance < 5., "{camera:?}");
        assert_eq!((camera.yaw, camera.pitch), (0.6, 0.45));
        editor
            .action("editor.camera", json!({"target":[50,0,50]}))
            .unwrap();
        editor.action("editor.frame", json!({})).unwrap();
        assert!(editor.state.camera.target.distance(crate_centre) < 1e-4);
        // Selected terrain or ground is framed.
        editor
            .action("editor.select", json!({"entities":["e_ground01"]}))
            .unwrap();
        editor.action("editor.frame", json!({})).unwrap();
        assert!(editor.state.camera.target.length() < 1e-4);
        assert!(editor.state.camera.distance > 200.);

        // The follow rig's view, as Play places it, pivoting on the aim point.
        editor
            .action("editor.scene.open", json!({"scene":"levels/rig.scene"}))
            .unwrap();
        let camera = editor.state.camera;
        let aim = Vec3::new(10., 1.5, 4.);
        let (yaw, pitch) = (30f32.to_radians(), 15f32.to_radians());
        let forward = Vec3::new(
            yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        assert!(camera.target.distance(aim) < 1e-4, "{camera:?}");
        assert!(camera.position().distance(aim - forward * 4.) < 1e-4);
        assert_eq!(camera.fov_y_degrees, 46.);

        // The rig's boom stops in front of the wall (1.9 m away, less 0.25 m).
        editor
            .action("editor.scene.open", json!({"scene":"levels/wall.scene"}))
            .unwrap();
        let camera = editor.state.camera;
        assert!((camera.distance - 1.65).abs() < 1e-3, "{camera:?}");
        assert!(camera.position().z > -1.9, "{:?}", camera.position());
    }

    // The last camera per scene is a user preference (isolated under target/).
    let profile = ara_editor::settings::config_dir()
        .unwrap()
        .join(format!("scene-cameras-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&profile);
    let preferences = profile.join("editor-settings.json");
    let saved = {
        let mut editor =
            ara_editor::Editor::open_with(&project.0, Some(preferences.clone())).unwrap();
        editor
            .action(
                "editor.camera",
                json!({"target":[1,2,3],"yaw":1.2,"pitch":0.3,"distance":7,"fovYDegrees":54}),
            )
            .unwrap();
        editor.state.camera
    };
    assert!(profile.join("scene-cameras.json").is_file());
    let editor = ara_editor::Editor::open_with(&project.0, Some(preferences)).unwrap();
    assert_eq!(editor.state.camera, saved);
    assert_eq!(editor.state.camera.fov_y_degrees, 54.);
    drop(editor);
    std::fs::remove_dir_all(&profile).unwrap();
}

/// Gizmo handles compose just the selection's transforms; each matches the whole
/// scene's composition, through nested, rotated and scaled parents.
#[test]
fn one_entity_world_transform_matches_the_whole_scene() {
    let project = Project::new();
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{
            "e_root0001":{"name":"Root","Transform":{"position":[1,2,3],"rotation":[0,45,0],"scale":[2,2,2]}},
            "e_mid00001":{"name":"Mid","parent":"e_root0001","Transform":{"position":[0,1,0],"rotation":[30,0,10]}},
            "e_leaf0001":{"name":"Leaf","parent":"e_mid00001","Transform":{"position":[0.5,0,-1],"scale":[1,3,1]}},
            "e_orphan01":{"name":"Orphan","parent":"e_missing1","Transform":{"position":[4,0,0]}}}}),
    );
    let editor = project.open();
    let view = editor.view().unwrap();
    let entities = ara_editor::world::EditWorldView::entities(&view);
    let all = ara_editor::world::world_transforms(entities);
    for (id, expected) in &all {
        let one = ara_editor::world::world_transform(entities, id).unwrap();
        assert!(
            one.abs_diff_eq(*expected, 1e-5),
            "{id}: {one} vs {expected}"
        );
    }
    assert_eq!(all.len(), 4);
}

/// A large group (a terrain's 144 chunks) starts collapsed so the scene's content
/// stays in view; expanding it is remembered, and the status bar counts entities.
#[test]
fn large_hierarchy_groups_start_collapsed() {
    let project = Project::new();
    let mut records = serde_json::Map::new();
    records.insert("e_terrain".into(), json!({"name":"Terrain","Transform":{}}));
    for n in 0..40 {
        records.insert(
            format!("e_chunk{n:03}"),
            json!({"name":"Terrain chunk","parent":"e_terrain"}),
        );
    }
    records.insert(
        "e_crate001".into(),
        json!({"name":"Salvage crate","Transform":{},"MeshRenderer":{}}),
    );
    write(
        &project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/main.scene"),
        json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records": records}),
    );
    let mut editor = project.open();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    assert_eq!(
        shell.hierarchy_rows(),
        2,
        "Terrain (collapsed) and the crate"
    );
    let docs = shell.ui.find("status-docs").unwrap();
    assert_eq!(
        shell.ui.nodes[docs].text.as_ref().unwrap().value,
        "42 entities"
    );
    // Expanding shows the chunks and stays expanded; collapsing again hides them.
    editor
        .action(
            "editor.collapse",
            json!({"entity":"e_terrain","enabled":false}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(shell.hierarchy_rows(), 42);
    editor
        .action(
            "editor.collapse",
            json!({"entity":"e_terrain","enabled":true}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(shell.hierarchy_rows(), 2);
}
