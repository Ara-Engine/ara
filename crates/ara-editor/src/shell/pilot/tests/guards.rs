use super::*;

#[test]
fn deleting_the_pilot_target_cannot_be_overwritten_at_release() {
    let project=Project::new();
    let (mut editor,mut shell)=open(&project);
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    shell.poll_pilot(&mut editor,true,false).unwrap();
    move_view(&mut editor);
    shell.poll_pilot(&mut editor,true,true).unwrap();
    editor.apply(vec![json!({"cmd":"entity.delete","args":{"entities":[CAMERA]}})],"Delete piloted camera",None).unwrap();
    let revision=editor.host.store().revision();
    let before=records(&editor);
    assert!(shell.poll_pilot(&mut editor,false,false).is_err());
    assert!(!shell.pilot_active());
    assert_eq!(editor.host.store().revision(),revision);
    assert_eq!(records(&editor),before);
    assert_eq!(shell.pilot_report()["last"]["authoringChanged"],false);
}
#[test]
fn active_agent_round_trip_on_camera_or_parent_stops_before_any_pilot_commit() {
    for entity in [CAMERA,PARENT] {
        let project=Project::new();
        let (mut editor,mut shell)=open(&project);
        shell.toggle_pilot(&mut editor,&Target::None).unwrap();
        shell.poll_pilot(&mut editor,true,false).unwrap();
        move_view(&mut editor);
        shell.poll_pilot(&mut editor,true,true).unwrap();
        let initial=records(&editor)[entity]["Transform"]["position"].clone();
        let session=agent(&mut editor);
        for value in [json!([99.,98.,97.]),initial] {
            agent_set(&mut editor,&session,entity,"Transform.position",value);
        }
        let revision=editor.host.store().revision();
        let before=records(&editor);
        let error=shell.poll_pilot(&mut editor,false,false).unwrap_err();
        assert!(error.to_string().contains("edited"),"{error}");
        assert!(!shell.pilot_active());
        assert_eq!(editor.host.store().revision(),revision);
        assert_eq!(records(&editor),before);
    }
}
#[test]
fn unrelated_agent_edit_is_preserved_while_the_native_camera_gesture_commits() {
    let project=Project::new();
    let (mut editor,mut shell)=open(&project);
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    let base=editor.host.store().revision();
    shell.poll_pilot(&mut editor,true,false).unwrap();
    move_view(&mut editor);
    shell.poll_pilot(&mut editor,true,true).unwrap();
    let session=agent(&mut editor);
    agent_set(&mut editor,&session,OTHER,"Transform.position",json!([8.,9.,10.]));
    assert!(shell.poll_pilot(&mut editor,false,false).unwrap());
    assert_eq!(records(&editor)[OTHER]["Transform"]["position"],json!([8.,9.,10.]));
    assert_eq!(editor.host.store().history().last().unwrap().base,base);
    assert!(shell.pilot_active());
}
#[test]
fn an_agent_view_jump_never_turns_into_a_camera_authoring_edit() {
    let project=Project::new();
    let (mut editor,mut shell)=open(&project);
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    let revision=editor.host.store().revision();
    let before=records(&editor);
    let view=move_view(&mut editor);
    assert!(shell.poll_pilot(&mut editor,false,false).is_err());
    assert!(!shell.pilot_active());
    assert_eq!(editor.host.store().revision(),revision);
    assert_eq!(records(&editor),before);
    assert_eq!(editor.state.camera,view,"An unowned view update is not overwritten by pilot cancellation");
}
#[test]
fn invalid_projection_radius_follow_rig_roll_and_parent_scale_are_explicitly_rejected() {
    for case in ["ortho","radius","follow","roll","singular"] {
        let project=Project::new();
        let (mut editor,mut shell)=open(&project);
        match case {
            "ortho"=>{editor.action("editor.camera",json!({"projection":"orthographic"})).unwrap();}
            "radius"=>editor.state.camera.distance=0.,
            "follow"=>{editor.apply(vec![json!({"cmd":"component.add","args":{"entity":CAMERA,"component":"FollowCamera"}})],"Add unsupported follow rig fixture",None).unwrap();}
            "roll"=>{editor.apply(vec![json!({"cmd":"transform.set","args":{"entities":[CAMERA],"rotation":[10.,0.,20.]}})],"Rolled camera fixture",None).unwrap();}
            _=>{editor.apply(vec![json!({"cmd":"transform.set","args":{"entities":[PARENT],"scale":[0.,1.,1.]}})],"Singular parent fixture",None).unwrap();}
        }
        let revision=editor.host.store().revision();
        let before=records(&editor);
        assert!(shell.toggle_pilot(&mut editor,&Target::None).is_err(),"{case}");
        assert!(!shell.pilot_active(),"{case}");
        assert_eq!(editor.host.store().revision(),revision,"{case}");
        assert_eq!(records(&editor),before,"{case}");
    }
}
#[test]
fn nonuniform_parent_rotation_that_would_shear_cancels_without_changing_scale() {
    let project=Project::new();
    let (mut editor,mut shell)=open(&project);
    editor.apply(vec![
        json!({"cmd":"transform.set","args":{"entities":[CAMERA],"rotation":[0.,0.,0.]}}),
        json!({"cmd":"transform.set","args":{"entities":[PARENT],"scale":[2.,3.,4.]}}),
    ],"Finite nonuniform parent fixture",None).unwrap();
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    let baseline=editor.state.camera;
    let before=records(&editor);
    let revision=editor.host.store().revision();
    shell.poll_pilot(&mut editor,true,false).unwrap();
    move_view(&mut editor);
    shell.poll_pilot(&mut editor,true,true).unwrap();
    assert!(shell.poll_pilot(&mut editor,false,false).is_err());
    assert_eq!(editor.state.camera,baseline);
    assert_eq!(editor.host.store().revision(),revision);
    assert_eq!(records(&editor),before);
}

#[test]
fn switching_projection_during_a_pilot_gesture_restores_the_complete_pending_view() {
    let project=Project::new();
    let (mut editor,mut shell)=open(&project);
    shell.toggle_pilot(&mut editor,&Target::None).unwrap();
    let baseline=editor.state.camera;
    let before=records(&editor);
    let revision=editor.host.store().revision();
    shell.poll_pilot(&mut editor,true,false).unwrap();
    move_view(&mut editor);
    shell.poll_pilot(&mut editor,true,true).unwrap();
    editor.action("editor.camera",json!({"projection":"orthographic"})).unwrap();
    assert!(shell.poll_pilot(&mut editor,true,true).is_err());
    assert!(!shell.pilot_active());
    assert_eq!(editor.state.camera,baseline);
    assert_eq!(editor.host.store().revision(),revision);
    assert_eq!(records(&editor),before);
}
