//! Current-state collection patches and real retained Inspector input.
use super::*;
use crate::Editor;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = crate::test_target().join("collection-tests").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        let list: Vec<_> = (0..160).map(|index| format!("item{index}")).collect();
        let map: serde_json::Map<String, Value> = (0..160)
            .map(|index| (format!("k{index:03}"), json!(index)))
            .collect();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Collection edits","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_collect1","records":{
                    "e_collect01":{"Transform":{},"MeshRenderer":{"emissiveMaterials":list},"AudioGraphSource":{"parameters":map},"Tilemap":{"layers":[{"id":"l_one","order":1},{"id":"l_two","order":2}]}},
                    "e_collect02":{"Transform":{},"MeshRenderer":{"emissiveMaterials":["other0","other1"]},"AudioGraphSource":{"parameters":{"k000":900,"private":42}},"Tilemap":{"layers":[{"id":"l_two","order":20},{"id":"l_one","order":10}]}}
                }}),
            ),
        ] {
            std::fs::write(
                root.join(
                    if path == "ara.project.json" || path.starts_with("settings/") {
                        std::path::PathBuf::from(path)
                    } else {
                        std::path::PathBuf::from(ara_doc::AUTHORING_ROOT).join(path)
                    },
                ),
                ara_json::to_canonical_string(&value),
            )
            .unwrap();
        }
        Self(root)
    }
    fn open(&self) -> Editor {
        Editor::open(&self.0).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn record(editor: &Editor, entity: &str) -> Value {
    json!(
        editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .unwrap()
            .record(entity)
            .unwrap()
    )
}
fn call(entity: &str, path: &str, pointer: &str, operation: Operation, value: Value) -> Value {
    json!({"cmd":"editor.property.nested.edit","args":{"entity":entity,"path":path,"pointer":pointer,"operation":operation,"value":value}})
}
fn skipped(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(map.get("skipped") == Some(&Value::Bool(true)))
                + map.values().map(skipped).sum::<usize>()
        }
        Value::Array(values) => values.iter().map(skipped).sum(),
        _ => 0,
    }
}

#[test]
fn item_one_hundred_edits_matching_objects_and_undoes_as_one_transaction() {
    let project = Project::new();
    let mut editor = project.open();
    let result = editor
        .apply(
            ["e_collect01", "e_collect02"]
                .into_iter()
                .map(|entity| {
                    call(
                        entity,
                        "MeshRenderer.emissiveMaterials",
                        "/100",
                        Operation::Set,
                        json!("edited"),
                    )
                })
                .collect(),
            "Edit item 100",
            None,
        )
        .unwrap();
    assert_eq!(
        record(&editor, "e_collect01")["MeshRenderer"]["emissiveMaterials"][100],
        "edited"
    );
    assert_eq!(
        record(&editor, "e_collect01")["MeshRenderer"]["emissiveMaterials"][99],
        "item99"
    );
    assert_eq!(
        record(&editor, "e_collect02")["MeshRenderer"]["emissiveMaterials"],
        json!(["other0", "other1"])
    );
    assert_eq!(skipped(&result), 1);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(
        record(&editor, "e_collect01")["MeshRenderer"]["emissiveMaterials"][100],
        "item100"
    );
}

#[test]
fn append_remove_rename_and_invalid_paths_remain_typed_and_atomic() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .apply(
            vec![call(
                "e_collect02",
                "MeshRenderer.emissiveMaterials",
                "",
                Operation::Append,
                json!("new"),
            )],
            "Append",
            None,
        )
        .unwrap();
    assert_eq!(
        record(&editor, "e_collect02")["MeshRenderer"]["emissiveMaterials"],
        json!(["other0", "other1", "new"])
    );
    editor
        .apply(
            vec![
                call(
                    "e_collect02",
                    "MeshRenderer.emissiveMaterials",
                    "/1",
                    Operation::Remove,
                    Value::Null,
                ),
                call(
                    "e_collect02",
                    "AudioGraphSource.parameters",
                    "/private",
                    Operation::Rename,
                    json!("renamed"),
                ),
            ],
            "Remove and rename",
            None,
        )
        .unwrap();
    assert_eq!(
        record(&editor, "e_collect02")["AudioGraphSource"]["parameters"]["renamed"],
        42
    );
    let before = record(&editor, "e_collect02");
    assert!(
        editor
            .apply(
                vec![call(
                    "e_collect02",
                    "AudioGraphSource.parameters",
                    "not-a-pointer",
                    Operation::Set,
                    json!(2)
                )],
                "Invalid pointer",
                None
            )
            .is_err()
    );
    assert_eq!(record(&editor, "e_collect02"), before);
}
