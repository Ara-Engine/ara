//! Real typed history tests; no player, native window, GPU or external editor is launched.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_proto::Endpoint;
use glam::{Mat4,Vec3};
use std::{path::PathBuf,sync::atomic::{AtomicU64,Ordering}};
mod guards;
mod benchmark;

const CAMERA:&str="e_pilotcam";
const PARENT:&str="e_pilotparent";
const OTHER:&str="e_pilotother";
struct Project(PathBuf);
impl Project {
    fn new()->Self {
        static NEXT:AtomicU64=AtomicU64::new(0);
        let root=std::path::absolute(crate::test_target()).unwrap().join("camera-pilot")
            .join(format!("{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        for (path,value) in [
            (root.join("ara.project.json"),json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Pilot","main":"levels/main.scene"})),
            (root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),json!({"$schema":"ara://schema/scene/1","id":"sc_pilot","records":{}})),
        ] {std::fs::write(path,ara_json::to_canonical_string(&value)).unwrap();}
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::path::absolute(crate::test_target()).unwrap()));
        let _=std::fs::remove_dir_all(&self.0);
    }
}
fn open(project:&Project)->(Editor,Shell) {
    let mut editor=Editor::open_with(&project.0,Some(project.0.join(".ara/test-settings.json"))).unwrap();
    editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":PARENT,"components":{"Transform":{"position":[3.,2.,-1.],"rotation":[0.,30.,0.],"scale":[2.,2.,2.]}}}}),
        json!({"cmd":"entity.create","args":{"id":CAMERA,"name":"Piloted Camera","parent":PARENT,"components":{"Transform":{"position":[1.,2.,3.],"rotation":[10.,0.,0.],"scale":[1.2,1.2,1.2]},"Camera":{"fov":55.,"priority":7}}}}),
        json!({"cmd":"entity.create","args":{"id":OTHER,"components":{"Transform":{}}}}),
    ],"Pilot test fixture",None).unwrap();
    editor.action("editor.select",json!({"entities":[CAMERA]})).unwrap();
    editor.action("editor.camera",json!({"projection":"perspective"})).unwrap();
    let shell=Shell::new(&mut editor,[1440.,900.]).unwrap();
    (editor,shell)
}
fn records(editor:&Editor)->Value {
    json!(editor.host.store().workspace().doc(&editor.state.doc).unwrap().records())
}
fn set_view(editor:&mut Editor,view:Camera) {
    restore(editor,view).unwrap();
}
fn world(editor:&Editor,id:&str)->Mat4 {
    crate::world::world_transform(&crate::world::resolve(editor.host.store(),&editor.state.doc).unwrap(),id).unwrap()
}
fn move_view(editor:&mut Editor)->Camera {
    let mut view=editor.state.camera;
    view.fly(Vec3::new(1.,0.,-1.),1.25);
    view.look([20.,-12.]);
    view.set_fov(65.);
    set_view(editor,view);
    view
}
fn agent(editor:&mut Editor)->String {
    Endpoint::call(&mut editor.host,"session.begin",json!({"client":"pilot-guard-test","author":"agent:pilot-proof"})).unwrap()["session"].as_str().unwrap().to_owned()
}
fn agent_set(editor:&mut Editor,session:&str,entity:&str,path:&str,value:Value) {
    Endpoint::call(&mut editor.host,"cmd.apply",json!({"session":session,"doc":editor.state.doc,"commands":[{"cmd":"property.set","args":{"entity":entity,"path":path,"value":value}}]})).unwrap();
    editor.sync();
}

#[test]
fn pilot_navigation_batches_parent_inverse_pose_and_fov_into_one_undo() {
    let project=Project::new();
    let (mut editor,mut shell)=open(&project);
    let before=records(&editor);
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    let revision=editor.host.store().revision();
    shell.poll_pilot(&mut editor,true,false).unwrap();
    let view=move_view(&mut editor);
    for _ in 0..12 {shell.poll_pilot(&mut editor,true,true).unwrap();}
    assert_eq!(editor.host.store().revision(),revision,"held navigation remains transient");
    assert_eq!(records(&editor),before);
    assert!(shell.poll_pilot(&mut editor,false,false).unwrap());
    assert_eq!(editor.host.store().revision(),revision+1);
    let actual=world(&editor,CAMERA);
    assert!(actual.transform_point3(Vec3::ZERO).distance(view.position())<0.001);
    assert!((actual.to_scale_rotation_translation().1*Vec3::NEG_Z-(view.target-view.position()).normalize()).length()<0.001);
    let after=records(&editor);
    assert_eq!(after[CAMERA]["Camera"],json!({"fov":65.,"priority":7}));
    assert_eq!(after[CAMERA]["Transform"]["scale"],before[CAMERA]["Transform"]["scale"]);
    assert_eq!(after[PARENT],before[PARENT]);
    assert_eq!(editor.host.store().history().last().unwrap().why.as_deref(),Some("Pilot camera navigation"));
    editor.action("history.undo",json!({})).unwrap();
    assert_eq!(records(&editor),before);
    editor.action("history.redo",json!({})).unwrap();
    assert_eq!(records(&editor),after);
}
#[test]
fn escape_restores_the_gesture_view_without_authoring_or_unpiloting() {
    let project=Project::new();
    let (mut editor,mut shell)=open(&project);
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    let view=editor.state.camera;
    let before=records(&editor);
    let revision=editor.host.store().revision();
    shell.poll_pilot(&mut editor,true,false).unwrap();
    move_view(&mut editor);
    shell.poll_pilot(&mut editor,true,true).unwrap();
    assert!(shell.cancel_pilot_navigation(&mut editor).unwrap());
    assert_eq!(editor.state.camera,view);
    assert_eq!(records(&editor),before);
    assert_eq!(editor.host.store().revision(),revision);
    assert!(shell.pilot_active());
    assert!(!shell.poll_pilot(&mut editor,false,false).unwrap());
}
#[test]
fn explicit_unpilot_cancels_pending_view_and_keeps_the_last_committed_pose() {
    let project=Project::new();
    let (mut editor,mut shell)=open(&project);
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    let baseline=editor.state.camera;
    shell.poll_pilot(&mut editor,true,false).unwrap();
    move_view(&mut editor);
    shell.poll_pilot(&mut editor,true,true).unwrap();
    let revision=editor.host.store().revision();
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    assert!(!shell.pilot_active());
    assert_eq!(editor.state.camera,baseline);
    assert_eq!(editor.host.store().revision(),revision);
}

#[test]
fn discrete_owned_fov_change_commits_once_without_touching_the_camera_transform() {
    let project=Project::new();
    let (mut editor,mut shell)=open(&project);
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    let revision=editor.host.store().revision();
    let before=records(&editor);
    editor.action("editor.camera",json!({"fovYDegrees":70.})).unwrap();
    assert!(shell.poll_pilot(&mut editor,false,true).unwrap());
    assert_eq!(editor.host.store().revision(),revision+1);
    let after=records(&editor);
    assert_eq!(after[CAMERA]["Transform"],before[CAMERA]["Transform"]);
    assert_eq!(after[CAMERA]["Camera"],json!({"fov":70.,"priority":7}));
    for _ in 0..8 {assert!(!shell.poll_pilot(&mut editor,false,false).unwrap());}
    assert_eq!(editor.host.store().revision(),revision+1);
    editor.action("history.undo",json!({})).unwrap();
    assert_eq!(records(&editor),before);
}
