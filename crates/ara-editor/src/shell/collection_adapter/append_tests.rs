//! Explicit Add reveals its own item; ordinary model notifications preserve selection.
#![allow(clippy::unwrap_used)]
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let root = target.join("collection-append").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Collection append","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_append001","records":{"e_append001":{"name":"Mesh","Transform":{},"MeshRenderer":{}}}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn slots(editor: &Editor) -> Value {
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    resolved["e_append001"].record["MeshRenderer"]["materialOverrides"].clone()
}

#[test]
fn explicit_append_after_a_background_insert_reveals_item_102_in_a_bounded_pool_and_undo_keeps_the_background_item()
 {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let material = editor
        .action(
            "editor.resource.create",
            json!({"path":"materials/steel.graph","domain":"material"}),
        )
        .unwrap();
    let reference = format!(
        "@materials/steel.graph#{}",
        material["doc"].as_str().unwrap()
    );
    let initial: Vec<_> = (0..100)
        .map(|primitive| json!({"primitive":primitive,"material":reference}))
        .collect();
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_append001","path":"MeshRenderer.materialOverrides","value":initial}})],"Initial collection",None).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_append001"]}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
    let field = "MeshRenderer.materialOverrides";
    let scroll = id(field, "");
    let selected = shell.collections[&scroll].selected.clone();
    assert_eq!(selected, Some(ItemKey::Index(0)));
    // The Add picker remembers the semantic destination before another writer appends.
    let target = format!("{field}#/-/material");
    assert!(shell.appends(&target));
    let background = commands(
        &editor,
        &editor.state.selection,
        &shell.fields[field],
        &slots(&editor),
        "",
        Operation::Append,
        json!({"primitive":100,"material":reference}),
        None,
    )
    .unwrap();
    editor.apply(background, "Background append", None).unwrap();
    let before_local = slots(&editor);
    assert_eq!(before_local.as_array().unwrap().len(), 101);
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert_eq!(
        shell.collections[&scroll].selected, selected,
        "background notifications cannot select their inserted item"
    );
    let local = shell
        .field_commands(&editor, &editor.state.selection, &target, json!(reference))
        .unwrap();
    assert_eq!(local[0]["args"]["operation"], "append");
    assert_eq!(local[0]["args"]["pointer"], "");
    let revision = editor.host.store().revision();
    editor.apply(local, "Explicit picker Add", None).unwrap();
    assert!(
        shell
            .reveal_appended_collection(&editor, field, "")
            .unwrap()
    );
    shell.rebuild(&mut editor).unwrap();
    shell.reveal_collection_appends(&editor).unwrap();
    shell.frame().unwrap();
    let after = slots(&editor);
    assert_eq!(after.as_array().unwrap().len(), 102);
    assert_eq!(
        &after.as_array().unwrap()[..101],
        before_local.as_array().unwrap().as_slice()
    );
    assert_eq!(
        shell.collections[&scroll].selected,
        Some(ItemKey::Index(101))
    );
    assert!(shell.collections[&scroll].range.contains(&101));
    assert!(
        shell.collections[&scroll].range.len() <= 14,
        "only the viewport and overscan are mounted"
    );
    let container = shell.shown_node(&scroll).unwrap();
    assert!(shell.ui.scroll_position(container).unwrap()[1] > 0.);
    let detail = shell
        .shown_node("nest-edit-MeshRenderer-materialOverrides-101-primitive")
        .unwrap();
    assert_eq!(
        shell.input.focused(),
        Some(ara_ui::source::stable_id(
            &shell.ui.nodes[detail].element.id
        ))
    );
    assert!(
        shell.ui.nodes[detail]
            .rect
            .intersection(shell.ui.nodes[detail].clip)
            .h
            > 0.
    );
    assert_eq!(editor.host.store().revision(), revision + 1);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(slots(&editor), before_local);
}
