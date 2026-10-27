use super::*;

pub(super) fn create(editor: &mut Editor) -> ara_ui::Result<()> {
    editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":SOURCE,"name":"Snap source","components":{"Transform":{"position":[-4,7,0]},"MeshRenderer":{"size":[1,2,1],"color":[0.2,0.5,0.8,1]}}}}),
        json!({"cmd":"entity.create","args":{"id":CHILD,"parent":SOURCE,"name":"Selected child follows","components":{"Transform":{"position":[0,1,0]},"MeshRenderer":{"size":[0.4,0.4,0.4]}}}}),
        json!({"cmd":"entity.create","args":{"id":TARGET,"name":"Raised sloped target","components":{"Transform":{"position":[4,3,0],"rotation":[0,0,20]},"MeshRenderer":{"shape":"plane","size":[8,1,8],"color":[0.6,0.35,0.15,1]}}}}),
        json!({"cmd":"entity.create","args":{"id":EMPTY,"name":"Unsupported drop root","components":{"Transform":{"position":[100,8,0]}}}}),
    ],"Create native snap fixtures",None)?;
    Ok(())
}
pub(super) fn records(editor: &Editor) -> ara_ui::Result<Value> {
    Ok(json!(
        editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .ok_or("Snap fixture document missing")?
            .records()
    ))
}

impl SnapCheck {
    pub(super) fn setup(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
    ) -> ara_ui::Result<()> {
        shell.leave();
        shell.desktop_pointer = None;
        shell.menu = None;
        editor.action("editor.layout.reset", json!({}))?;
        editor.action("editor.focus", json!({"panel":"scene"}))?;
        editor.action("editor.select",json!({"entities":if act==Act::DropInvalid {vec![SOURCE,CHILD,EMPTY]}else{vec![SOURCE,CHILD]}}))?;
        editor.action(
            "editor.tool",
            json!({"tool":"translate","local":false,"snapping":true,"moveSnap":5.}),
        )?;
        editor.action("editor.view", json!({"gizmos":true,"gameView":false}))?;
        editor.action(
            "editor.settings.set",
            json!({"key":"viewport.surfaceSnap","value":!act.vertex()}),
        )?;
        editor.action(
            "editor.settings.set",
            json!({"key":"viewport.vertexSnap","value":false}),
        )?;
        editor.action(
            "editor.settings.set",
            json!({"key":"viewport.surfaceOffset","value":if act.vertex(){0.}else{0.125}}),
        )?;
        editor.action("editor.camera",json!({"target":[0,3,0],"yaw":0.,"pitch":0.45,"distance":24.,"fovYDegrees":50.,"projection":if act==Act::Ortho {"orthographic"}else{"perspective"},"orthographicHeight":18.}))?;
        editor.apply(vec![json!({"cmd":"transform.set","args":{"entities":[SOURCE],"position":if act.drop(){[4,9,0]}else{[-4,7,0]},"rotation":[0,0,0],"scale":[1,1,1]}})],"Reset native snap fixture",None)?;
        let view = editor.view()?;
        let hidden: Vec<_> = view
            .entities()
            .keys()
            .filter(|id| {
                id.as_str() != SOURCE
                    && id.as_str() != CHILD
                    && id.as_str() != TARGET
                    && id.as_str() != EMPTY
            })
            .cloned()
            .collect();
        for id in hidden {
            if !editor.state.hidden.contains(&id) {
                editor.action("editor.visibility", json!({"entity":id,"enabled":false}))?;
            }
        }
        if act == Act::Floating {
            editor.action(
                "editor.layout.move",
                json!({"panel":"scene","slot":"float","rect":[180,140,1000,700]}),
            )?;
        }
        shell.rebuild(editor)?;
        shell.input.blur(&mut shell.ui);
        self.revision = editor.host.store().revision();
        self.before = records(editor)?;
        self.source = None;
        self.target = None;
        self.normal = None;
        self.vertices.clear();
        self.delta = Vec3::ZERO;
        Ok(())
    }
}
