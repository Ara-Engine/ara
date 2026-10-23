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
fn agent_presence_does_not_take_the_human_camera_and_deleted_records_remain_reviewable(){
    let project=Project::new();let mut editor=project.open();editor.action("editor.select",json!({"entities":["e_child001"]})).unwrap();let human=editor.state.camera;
    let agent=Endpoint::call(&mut editor,"session.begin",json!({"client":"presence","author":"agent:presence"})).unwrap()["session"].as_str().unwrap().to_owned();
    Endpoint::call(&mut editor,"editor.select",json!({"session":agent,"entities":["e_crate001"]})).unwrap();Endpoint::call(&mut editor,"editor.camera",json!({"session":agent,"target":[3,2,1],"yaw":1.,"pitch":0.2,"distance":6.})).unwrap();assert_eq!(editor.state.camera,human);assert_eq!(editor.state.selection,["e_child001"]);assert_eq!(editor.state.presence[&agent].selection,["e_crate001"]);
    editor.action("editor.follow",json!({"agent":agent})).unwrap();assert_eq!(editor.state.camera.distance,6.);editor.action("editor.follow",json!({})).unwrap();
    let result=Endpoint::call(&mut editor,"cmd.apply",json!({"session":agent,"doc":"sc_test0001","why":"Remove obsolete crate","commands":[{"cmd":"entity.delete","args":{"entities":["e_crate001"]}}]})).unwrap();let view=editor.view().unwrap();assert!(view.entities().is_empty());let scene=view.scene(&project.0,&editor.state,None).unwrap();assert_eq!(scene.meshes.len(),1);assert!(scene.meshes[0].name.contains("deleted"));assert_eq!(scene.materials[scene.meshes[0].material].flags[0],2);
    editor.action("review.revert",json!({"txn":result["txn"]})).unwrap();assert_eq!(editor.view().unwrap().entities().len(),2);
}

#[test]
fn duplicate_reparent_and_reset_are_one_command_each_with_undo(){
    let project=Project::new();let mut editor=project.open();let result=editor.apply(vec![json!({"cmd":"editor.entity.duplicate","args":{"entity":"e_crate001"}})],"Duplicate crate",None).unwrap();assert_eq!(editor.view().unwrap().entities().len(),4);let id=result["outputs"][0]["id"].as_str().unwrap();assert_eq!(editor.view().unwrap().entities()[id].record["name"],"Salvage crate copy");editor.action("history.undo",json!({})).unwrap();assert_eq!(editor.view().unwrap().entities().len(),2);
    editor.apply(vec![json!({"cmd":"entity.reparent","args":{"entity":"e_child001"}}),json!({"cmd":"property.set","args":{"entity":"e_crate001","path":"Transform.position.x","value":3}})],"Reparent and position",None).unwrap();editor.apply(vec![json!({"cmd":"editor.property.reset","args":{"entity":"e_crate001","path":"Transform.position"}})],"Reset",None).unwrap();assert_eq!(x(&editor),0.);editor.action("history.undo",json!({})).unwrap();assert_eq!(x(&editor),3.);
}
