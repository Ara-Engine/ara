//! Atomic creation+pose application, cross-root references, linked prefabs and rollback.
#![allow(clippy::unwrap_used)]
use crate::Editor;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn open() -> Editor {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ux-duplicate-tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    std::fs::create_dir_all(root.join("levels")).unwrap();
    std::fs::create_dir_all(root.join("prefabs")).unwrap();
    for (path, value) in [
        (
            "ara.project.json",
            json!({"name":"Duplicate gesture","main":"levels/main.scene"}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_duptest1","records":{}}),
        ),
        (
            "prefabs/unit.prefab",
            json!({"$schema":"ara://schema/prefab/1","id":"pf_duptest1","records":{"e_root01":{"name":"Linked root","Transform":{},"FollowCamera":{}},"e_inner01":{"name":"Linked child","parent":"e_root01","Transform":{}}}}),
        ),
    ] {
        std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
    }
    Editor::open(&root).unwrap()
}
#[test]
fn duplicate_transform_is_one_undo_and_remaps_references_across_selected_subtrees() {
    let mut editor = open();
    editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":"e_sourcea1","name":"A","components":{"Transform":{},"FollowCamera":{"target":"e_sourcec1"}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_sourceb1","name":"B","parent":"e_sourcea1","components":{"Transform":{}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_sourcec1","name":"C","components":{"Transform":{},"FollowCamera":{"target":"e_sourceb1"}}}}),
    ],"Create linked sources",None).unwrap();
    let before = editor.host.store().revision();
    editor.apply(vec![json!({"cmd":"editor.entities.duplicate","args":{"entities":["e_sourcea1","e_sourceb1","e_sourcec1"],"poses":[{"entity":"e_sourcea1","position":[3,4,5],"rotation":[0,30,0],"scale":[1,1,1]},{"entity":"e_sourcec1","position":[-1,2,0],"rotation":[0,0,0],"scale":[1,1,1]}]}})],"Duplicate drag",Some(before)).unwrap();
    assert_eq!(editor.host.store().revision(), before + 1);
    let entities = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert_eq!(entities.len(), 6);
    let copied = |name: &str| {
        entities
            .iter()
            .find(|(_, entity)| {
                entity.record.get("name").and_then(|name| name.as_str()) == Some(name)
            })
            .map(|(id, _)| id.as_str())
            .unwrap()
    };
    let a = copied("A copy");
    let c = copied("C copy");
    let b = entities
        .iter()
        .find(|(_, entity)| entity.parent.as_deref() == Some(a))
        .map(|(id, _)| id.as_str())
        .unwrap();
    assert_eq!(entities[a].record["FollowCamera"]["target"], c);
    assert_eq!(entities[c].record["FollowCamera"]["target"], b);
    assert_eq!(
        entities[a].record["Transform"]["position"],
        json!([3, 4, 5])
    );
    assert_eq!(
        entities["e_sourcea1"].record["FollowCamera"]["target"],
        "e_sourcec1"
    );
    assert_eq!(
        crate::world::local_transform(&entities["e_sourcea1"].record)
            .w_axis
            .truncate(),
        glam::Vec3::ZERO
    );
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(
        crate::world::resolve(editor.host.store(), &editor.state.doc)
            .unwrap()
            .len(),
        3
    );
}
#[test]
fn linked_prefab_duplicate_keeps_source_and_internal_reference_without_flattened_children() {
    let mut editor = open();
    editor.apply(vec![json!({"cmd":"prefab.instantiate","args":{"id":"e_instance1","prefab":"@prefabs/unit.prefab#pf_duptest1"}}),json!({"cmd":"property.set","args":{"entity":"e_instance1","path":"FollowCamera.target","value":"e_instance1/e_inner01"}})],"Create linked instance",None).unwrap();
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    let original_children = resolved
        .keys()
        .filter(|id| id.starts_with("e_instance1/"))
        .count();
    let before = editor.host.store().revision();
    editor.apply(vec![json!({"cmd":"editor.entities.duplicate","args":{"entities":["e_instance1"],"poses":[{"entity":"e_instance1","position":[5,6,7],"rotation":[0,0,0],"scale":[1,1,1]}]}})],"Duplicate prefab drag",Some(before)).unwrap();
    let document = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .unwrap();
    assert_eq!(document.records().len(), 2);
    let copy = document
        .records()
        .keys()
        .find(|id| id.as_str() != "e_instance1")
        .unwrap()
        .as_str();
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert_eq!(
        resolved
            .keys()
            .filter(|id| id.starts_with(&format!("{copy}/")))
            .count(),
        original_children
    );
    assert_eq!(
        resolved[copy].record["FollowCamera"]["target"],
        format!("{copy}/e_inner01")
    );
    assert_eq!(
        resolved[copy].record["Transform"]["position"],
        json!([5, 6, 7])
    );
    assert_eq!(editor.host.store().revision(), before + 1);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(
        editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .unwrap()
            .records()
            .len(),
        1
    );
}
#[test]
fn invalid_root_pose_rolls_back_every_copy() {
    let mut editor = open();
    editor.apply(vec![json!({"cmd":"entity.create","args":{"id":"e_sourcea1","components":{"Transform":{}}}}),json!({"cmd":"entity.create","args":{"id":"e_sourceb1","parent":"e_sourcea1","components":{"Transform":{}}}})],"Create subtree",None).unwrap();
    let before = editor.host.store().revision();
    assert!(editor.apply(vec![json!({"cmd":"editor.entities.duplicate","args":{"entities":["e_sourcea1","e_sourceb1"],"poses":[{"entity":"e_sourceb1","position":[1,2,3],"rotation":[0,0,0],"scale":[1,1,1]}]}})],"Invalid duplicate",Some(before)).is_err());
    assert_eq!(editor.host.store().revision(), before);
    assert_eq!(
        editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .unwrap()
            .records()
            .len(),
        2
    );
}
