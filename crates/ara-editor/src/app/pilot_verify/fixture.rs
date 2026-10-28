//! Typed fixture setup is reported separately from measured native UI gestures.
use super::*;
pub(super) fn create(editor: &mut Editor) -> ara_ui::Result<Value> {
    editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":PARENT,"name":"Native Pilot parent","components":{"Transform":{"rotation":[0,30,0],"scale":[2,3,2]}}}}),
        json!({"cmd":"entity.create","args":{"id":CAMERA,"parent":PARENT,"name":"Native Pilot Camera","components":{"Transform":{"position":[0,2,6],"rotation":[0,0,0],"scale":[1,1,1]},"Camera":{"fov":55,"priority":7}}}}),
        json!({"cmd":"entity.create","args":{"id":BOX,"name":"Native bounds and box","components":{"Transform":{"position":[0,6,0],"rotation":[0,20,0],"scale":[1.3,1,0.8]},"MeshRenderer":{"size":[2,2,2],"color":[0.2,0.6,0.9,1]},"Collider":{"kind":"box","halfExtents":[1.1,1.2,1.3]}}}}),
        json!({"cmd":"entity.create","args":{"id":SPHERE,"name":"Native sphere sensor","components":{"Transform":{"position":[-3,6,-2]},"MeshRenderer":{"shape":"sphere","size":[1.5,1.5,1.5],"color":[0.9,0.5,0.2,1]},"Collider":{"kind":"sphere","radius":1,"sensor":true}}}}),
        json!({"cmd":"entity.create","args":{"id":COOKED,"name":"Unavailable cooked collider","components":{"Transform":{"position":[3,6,-2]},"MeshRenderer":{"size":[1,1,1]},"Collider":{"kind":"cooked"}}}}),
    ],"Native Pilot and diagnostic fixture setup (typed protocol; not native input)",None).map_err(Into::into)
}
pub(super) fn records(editor: &Editor) -> ara_ui::Result<Value> {
    let doc = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("Pilot fixture Scene missing")?;
    Ok(Value::Object(
        IDS.into_iter()
            .filter_map(|id| doc.record(id).map(|record| (id.into(), json!(record))))
            .collect(),
    ))
}
pub(super) fn setup(editor: &mut Editor, shell: &mut Shell, index: usize) -> ara_ui::Result<()> {
    shell.leave();
    editor.action("editor.palette", json!({"open":false}))?;
    editor.action("editor.layout.reset", json!({}))?;
    editor.action("editor.focus", json!({"panel":"scene"}))?;
    editor.action(
        "editor.view",
        json!({"hideOverlays":false,"gameView":false,"gizmos":true}),
    )?;
    editor.action(
        "editor.select",
        json!({"entities":if index==7{vec![BOX]}else{vec![CAMERA]}}),
    )?;
    if index == 0 || index == 7 {
        editor.action("editor.camera",json!({"projection":"perspective","target":[0,6,0],"yaw":0.45,"pitch":0.,"distance":15.,"fovYDegrees":55.}))?;
    }
    if index == 0 {
        editor.action("editor.follow", json!({}))?;
        for key in ["viewport.bounds", "viewport.colliders", "viewport.stats"] {
            editor.action("editor.settings.set", json!({"key":key,"value":false}))?;
        }
        editor.action(
            "editor.settings.set",
            json!({"key":"viewport.cameraSpeed","value":4.}),
        )?;
        let ids = editor
            .view()?
            .entities()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for id in ids {
            let hidden = !IDS.contains(&id.as_str());
            if editor.state.hidden.contains(&id) != hidden {
                editor.action("editor.visibility", json!({"entity":id,"enabled":hidden}))?;
            }
        }
    }
    shell.rebuild(editor)?;
    Ok(())
}
