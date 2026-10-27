use super::*;

pub(super) fn create(editor: &mut Editor) -> ara_ui::Result<()> {
    let mut commands: Vec<_> = (0..ROWS).map(|index| json!({"cmd":"entity.create","args":{"id":row(index),"name":format!("Selection proof {index:04}"),"index":index,"components":{"Transform":{}}}})).collect();
    for (index, id) in MESHES.into_iter().enumerate() {
        commands.push(json!({"cmd":"entity.create","args":{"id":id,"name":format!("Selection mesh {}",index+1),"components":{"Transform":{"position":[index as f32*4.-4.,0.,0.]},"MeshRenderer":{"size":[1,1,1]},"FollowCamera":{"target":MESHES[(index+1)%MESHES.len()]}}}}));
    }
    for (id, x) in [(HIDDEN, -2.), (LOCKED, -1.)] {
        commands.push(json!({"cmd":"entity.create","args":{"id":id,"name":id,"components":{"Transform":{"position":[x,0.,0.]},"MeshRenderer":{"size":[0.5,0.5,0.5]}}}}));
    }
    editor.apply(commands, "Create native selection fixtures", None)?;
    Ok(())
}
pub(super) fn records(editor: &Editor) -> ara_ui::Result<Value> {
    let document = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("Selection fixture document missing")?;
    Ok(json!(document.records()))
}
impl SelectionCheck {
    pub(super) fn setup(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
    ) -> ara_ui::Result<()> {
        shell.leave();
        shell.desktop_pointer = None;
        shell.menu = None;
        shell.dialog = None;
        editor.action("editor.palette", json!({"open":false}))?;
        editor.action("editor.layout.reset", json!({}))?;
        editor.action(
            "editor.search",
            json!({"panel":"hierarchy","query":if act.hierarchy(){"Selection proof"}else{""}}),
        )?;
        editor.action(
            "editor.focus",
            json!({"panel":if act.hierarchy(){"hierarchy"}else{"scene"}}),
        )?;
        editor.action(
            "editor.view",
            json!({"gizmos":act.duplicate(),"gameView":false}),
        )?;
        editor.action(
            "editor.tool",
            json!({"tool":"translate","local":false,"centre":false,"snapping":false}),
        )?;
        editor.action(
            "editor.camera",
            json!({"target":[0.,0.,0.],"yaw":0.,"pitch":0.,"distance":20.,"fovYDegrees":50.,"projection":if act==Act::MarqueeReverse{"orthographic"}else{"perspective"},"orthographicHeight":10.}),
        )?;
        let entities = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|error| format!("{error:?}"))?;
        for id in entities.keys() {
            let visible = MESHES.contains(&id.as_str()) || id == LOCKED;
            if editor.state.hidden.contains(id) == visible {
                editor.action("editor.visibility", json!({"entity":id,"enabled":!visible}))?;
            }
            if editor.state.locked.contains(id) != (id == LOCKED) {
                editor.action("editor.lock", json!({"entity":id,"enabled":id==LOCKED}))?;
            }
        }
        let selected: Vec<_> = match act {
            Act::MarqueeAdd | Act::MarqueeEscape => vec![MESHES[2]],
            Act::MarqueeSubtract => MESHES.to_vec(),
            Act::Duplicate | Act::DuplicateEscape => vec![MESHES[0], MESHES[1]],
            _ => Vec::new(),
        };
        editor.action("editor.select", json!({"entities":selected}))?;
        shell.hierarchy_offset = 0.;
        shell.rebuild(editor)?;
        if let Some(container) = shell.shown_node("hierarchy-rows") {
            shell.ui.scroll_to(container, [0., 0.])?;
        }
        shell.input.blur(&mut shell.ui);
        self.before_revision = editor.host.store().revision();
        self.before_records = records(editor)?;
        Ok(())
    }
}
