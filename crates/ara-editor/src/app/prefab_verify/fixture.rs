use super::*;

fn command(name: &str, args: Value) -> Value {
    json!({"cmd":name,"args":args})
}
fn source_records(editor: &Editor, doc: &str) -> ara_ui::Result<(String, String)> {
    let source = editor
        .host
        .store()
        .workspace()
        .doc(doc)
        .ok_or("J5 source document missing")?;
    let root = source
        .children(None)
        .first()
        .map(ToString::to_string)
        .ok_or("J5 source root missing")?;
    let child = source
        .children(Some(&root))
        .first()
        .map(ToString::to_string)
        .ok_or("J5 source child missing")?;
    Ok((root, child))
}
pub(super) fn create(editor: &mut Editor) -> ara_ui::Result<(String, String, String)> {
    editor.apply(
        vec![command(
            "doc.create",
            json!({"path":"levels/j5-proof.scene","id":SCENE}),
        )],
        "Create isolated J5 scene",
        None,
    )?;
    editor.action("editor.scene.open", json!({"scene":SCENE}))?;
    editor.apply(vec![
        command("entity.create",json!({"id":"e_j5base","name":"J5 base","components":{"Transform":{"position":[-4,0,0]},"MeshRenderer":{"size":[1,1,1]},"FollowCamera":{"target":null}}})),
        command("entity.create",json!({"id":"e_j5child","name":"J5 child","parent":"e_j5base","components":{"Transform":{"position":[0,2,0]},"MeshRenderer":{"size":[0.5,0.5,0.5]}}})),
        command("property.set",json!({"entity":"e_j5base","path":"FollowCamera.target","value":"e_j5child"})),
        command("prefab.create",json!({"entity":"e_j5base","path":"prefabs/j5-base.prefab","id":BASE})),
    ],"Create typed J5 base with an internal reference",None)?;
    let (base_root, base_child) = source_records(editor, BASE)?;
    editor.apply(vec![
        command("entity.create",json!({"id":"e_j5outer","name":"J5 outer","components":{"Transform":{"position":[4,0,0]}}})),
        command("prefab.instantiate",json!({"id":"e_j5inner","name":"J5 nested base","prefab":"@prefabs/j5-base.prefab#pf_j5base","parent":"e_j5outer"})),
        command("prefab.create",json!({"entity":"e_j5outer","path":"prefabs/j5-outer.prefab","id":OUTER})),
        command("prefab.instantiate",json!({"id":FIRST,"name":"J5 first","prefab":"@prefabs/j5-base.prefab#pf_j5base","position":[-2,0,0]})),
        command("prefab.instantiate",json!({"id":SECOND,"name":"J5 second","prefab":"@prefabs/j5-base.prefab#pf_j5base","position":[0,0,0]})),
        command("prefab.instantiate",json!({"id":VARIANT_ENTITY,"name":"J5 variant","prefab":"@prefabs/j5-base.prefab#pf_j5base","position":[2,0,0]})),
        command("prefab.instantiate",json!({"id":FAILURE,"name":"J5 invalid reference","prefab":"@prefabs/j5-base.prefab#pf_j5base","position":[0,0,4]})),
        command("entity.create",json!({"id":"e_j5external","name":"J5 scene-only target","components":{"Transform":{"position":[0,4,0]}}})),
    ],"Create typed J5 nested and repeated instances",None)?;
    let (_, nested_record) = source_records(editor, OUTER)?;
    Ok((
        base_root,
        base_child.clone(),
        format!("e_j5outer/{nested_record}/{base_child}"),
    ))
}

pub(super) fn snapshot(editor: &Editor) -> Value {
    let docs: serde_json::Map<String, Value> = editor
        .host
        .store()
        .workspace()
        .docs()
        .map(|(path, doc)| {
            (
                path.into(),
                Value::String(ara_doc::write(doc, editor.host.store().schema())),
            )
        })
        .collect();
    Value::Object(docs)
}

/// Capture every document's identity without retaining its body in each trace.
/// Full snapshots above still drive exact, all-document equality assertions.
pub(super) fn capture_documents(editor: &Editor) -> Value {
    let docs: serde_json::Map<String, Value> = editor
        .host
        .store()
        .workspace()
        .docs()
        .map(|(path, doc)| {
            let canonical = ara_doc::write(doc, editor.host.store().schema());
            (
                path.into(),
                json!({"canonicalBytes":canonical.len(),"blake3":blake3::hash(canonical.as_bytes()).to_hex().to_string()}),
            )
        })
        .collect();
    Value::Object(docs)
}

#[cfg(test)]
mod tests;

impl PrefabCheck {
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
        editor.action("editor.scene.open", json!({"scene":SCENE}))?;
        editor.action(
            "editor.settings.set",
            json!({"key":"input.keymap","value":"ara"}),
        )?;
        editor.action("editor.search", json!({"panel":"hierarchy","query":"J5"}))?;
        editor.action("editor.search", json!({"panel":"inspector","query":""}))?;
        editor.action("editor.focus", json!({"panel":"inspector"}))?;
        let entity = match act {
            Act::ApplyUndo => FIRST,
            Act::NestedNearest | Act::NestedExplicit => &self.nested,
            Act::VariantCreate | Act::VariantApply => VARIANT_ENTITY,
            Act::FailedApply => FAILURE,
        }
        .to_owned();
        if act == Act::VariantApply && editor.host.store().workspace().doc(VARIANT).is_none() {
            editor.apply(vec![command("prefab.variant",json!({"entity":VARIANT_ENTITY,"path":"prefabs/j5-variant-proof.prefab","id":VARIANT}))],"Set up linked J5 variant",None)?;
            editor.apply_to(
                BASE,
                vec![command(
                    "property.set",
                    json!({"entity":self.base_root,"path":"MeshRenderer.size","value":[2,3,4]}),
                )],
                "Change base inherited by J5 variant",
            )?;
        }
        let (path, value) = if act == Act::FailedApply {
            ("FollowCamera.target", json!("e_j5external"))
        } else {
            let factor = match act {
                Act::ApplyUndo => 2,
                Act::NestedNearest | Act::NestedExplicit => 3,
                Act::VariantCreate => 4,
                _ => 6,
            };
            ("Transform.scale", json!([factor, factor, factor]))
        };
        editor.apply(
            vec![command(
                "property.set",
                json!({"entity":entity,"path":path,"value":value}),
            )],
            "Set up J5 instance override",
            None,
        )?;
        editor.action("editor.select", json!({"entities":[entity]}))?;
        editor.action("editor.frame", json!({}))?;
        shell.hierarchy_offset = 0.;
        shell.rebuild(editor)?;
        shell.input.blur(&mut shell.ui);
        self.before = snapshot(editor);
        self.before_revision = editor.host.store().revision();
        self.before_history = editor.host.store().history().len();
        self.log_cursor = editor.action("logs", json!({"limit":1}))?["cursor"]
            .as_u64()
            .unwrap_or(0);
        Ok(())
    }
}
