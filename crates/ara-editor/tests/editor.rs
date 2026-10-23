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
        std::fs::create_dir_all(root.join("levels")).unwrap();
        write(
            &root.join("ara.project.json"),
            json!({"name":"Editor test"}),
        );
        write(
            &root.join("levels/main.scene"),
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
fn mcp_actions_agent_review_provenance_and_revert_are_live() {
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
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    assert!(shell.ui.find(&format!("turn-{txn}")).is_some());
    assert_eq!(
        shell.fields["Transform.position"].provenance,
        ara_editor::inspector::Provenance::AgentChanged
    );
    let mut clipboard = MemoryClipboard::default();
    shell
        .click(&format!("revert-{txn}"), &mut clipboard)
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
fn deferred_agent_batches_preserve_attribution_and_do_not_change_documents_before_approval() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action("editor.policy", json!({"policy":"onApproval"}))
        .unwrap();
    let session = Endpoint::call(
        &mut editor,
        "session.begin",
        json!({"client":"policy-test","author":"agent:policy"}),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let params = json!({"session":session,"doc":"sc_test0001","why":"Queued move","idempotencyKey":"policy-test-1","commands":[{"cmd":"transform.set","args":{"entities":["e_crate001"],"position":[7,0.6,0]}}]});
    let queued = Endpoint::call(&mut editor, "cmd.apply", params.clone()).unwrap();
    assert_eq!(queued["queued"], true);
    assert_eq!(x(&editor), 0.);
    assert_eq!(
        Endpoint::call(&mut editor, "cmd.apply", params).unwrap()["ticket"],
        queued["ticket"]
    );
    let second = Endpoint::call(
        &mut editor,
        "session.begin",
        json!({"client":"other-policy-test","author":"agent:other-policy"}),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = Endpoint::call(&mut editor,"cmd.apply",json!({"session":second,"doc":"sc_test0001","why":"Independent request","idempotencyKey":"policy-test-1","commands":[{"cmd":"transform.set","args":{"entities":["e_child001"],"position":[8,0,0]}}]})).unwrap();
    assert_ne!(
        queued["ticket"], other["ticket"],
        "idempotency keys belong to their session"
    );
    editor.reject(other["ticket"].as_str().unwrap()).unwrap();
    editor.tick();
    assert_eq!(x(&editor), 0.);
    editor.approve(queued["ticket"].as_str().unwrap()).unwrap();
    editor.tick();
    assert_eq!(x(&editor), 7.);
    assert_eq!(
        editor.host.store().history().last().unwrap().author,
        ara_ops::Author::Agent("policy".into())
    );
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
                (overlay::hit(&handles, p) == Some(axis)).then_some(p)
            })
            .expect("visible ring must be pickable");
        let point =
            overlay::plane_point(&state.camera, rect, ring, handles.pivot, handles.axes[axis])
                .unwrap();
        assert!((point - handles.pivot).dot(handles.axes[axis]).abs() < 0.001);
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
fn agent_presence_does_not_take_the_human_camera_and_deleted_records_remain_reviewable() {
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
    editor
        .action("editor.follow", json!({"agent":agent}))
        .unwrap();
    assert_eq!(editor.state.camera.distance, 6.);
    editor.action("editor.follow", json!({})).unwrap();
    let result=Endpoint::call(&mut editor,"cmd.apply",json!({"session":agent,"doc":"sc_test0001","why":"Remove obsolete crate","commands":[{"cmd":"entity.delete","args":{"entities":["e_crate001"]}}]})).unwrap();
    let view = editor.view().unwrap();
    assert!(view.entities().is_empty());
    let scene = view.scene(&project.0, &editor.state, None).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    assert!(scene.meshes[0].name.contains("deleted"));
    assert_eq!(scene.materials[scene.meshes[0].material].flags[0], 2);
    editor
        .action("review.revert", json!({"txn":result["txn"]}))
        .unwrap();
    assert_eq!(editor.view().unwrap().entities().len(), 2);
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
        &project.0.join("levels/main.scene"),
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
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("evidence/hierarchy-100k.json");
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    write(
        &output,
        json!({"entities":100000,"retainedNodes":node_count,"buildMs":build_ms,"scrollMs":scroll_ms,"idlePaintNodes":work.paint_nodes,"lastEntitySelected":true}),
    );
}
