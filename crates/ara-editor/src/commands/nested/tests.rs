//! Current-state collection patches and real retained Inspector input.
use super::*;
use crate::{Editor, shell::Shell};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"))
            .join("collection-tests")
            .join(format!(
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
fn planned_mixed_edit_preserves_later_authoritative_changes_and_map_values() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action(
            "editor.select",
            json!({"entities":["e_collect01","e_collect02"]}),
        )
        .unwrap();
    let shell = Shell::new(&mut editor, [1000., 900.]).unwrap();
    let calls = shell
        .field_commands(
            &editor,
            &editor.state.selection,
            "AudioGraphSource.parameters#/k000",
            json!(0.5),
        )
        .unwrap();
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_collect02","path":"AudioGraphSource.parameters[private]","value":73}})],"Concurrent unrelated change",None).unwrap();
    editor.apply(calls, "Edit matching map key", None).unwrap();
    assert_eq!(
        record(&editor, "e_collect01")["AudioGraphSource"]["parameters"]["k000"],
        json!(0.5)
    );
    let second = record(&editor, "e_collect02");
    assert_eq!(
        second["AudioGraphSource"]["parameters"],
        json!({"k000":0.5,"private":73})
    );
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(
        record(&editor, "e_collect02")["AudioGraphSource"]["parameters"],
        json!({"k000":900,"private":73})
    );
}

#[test]
fn keyed_items_resolve_by_id_across_different_orders_and_keep_their_values_when_moved() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action(
            "editor.select",
            json!({"entities":["e_collect01","e_collect02"]}),
        )
        .unwrap();
    let shell = Shell::new(&mut editor, [1000., 900.]).unwrap();
    let calls = shell
        .field_commands(
            &editor,
            &editor.state.selection,
            "Tilemap.layers#/0/order",
            json!(7),
        )
        .unwrap();
    editor.apply(calls, "Edit stable layer", None).unwrap();
    assert_eq!(
        record(&editor, "e_collect01")["Tilemap"]["layers"][0]["order"],
        7
    );
    assert_eq!(
        record(&editor, "e_collect02")["Tilemap"]["layers"][1]["order"],
        7
    );
    let mut moved = call(
        "e_collect02",
        "Tilemap.layers",
        "/0",
        Operation::MoveUp,
        Value::Null,
    );
    moved["args"]["hints"] = json!([{"depth":0,"key":"l_one","append":false}]);
    editor
        .apply(vec![moved], "Move stable layer", None)
        .unwrap();
    let second = record(&editor, "e_collect02");
    assert_eq!(second["Tilemap"]["layers"][0]["id"], "l_one");
    assert_eq!(second["Tilemap"]["layers"][0]["order"], 7);
    assert_eq!(second["Tilemap"]["layers"][1]["order"], 20);
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

#[derive(Default)]
struct Clip(String);
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}

#[test]
fn real_inspector_scroll_mounts_and_edits_item_one_hundred_with_a_bounded_pool() {
    use ara_ui::input::InputEvent;
    use ara_ui_text::EditEvent;
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action(
            "editor.select",
            json!({"entities":["e_collect01","e_collect02"]}),
        )
        .unwrap();
    editor
        .action("editor.view", json!({"maximized":"inspector"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1000., 900.]).unwrap();
    shell.offscreen = true;
    let field = "MeshRenderer.emissiveMaterials";
    let scroll = format!(
        "collection-{:016x}",
        ara_ui::source::stable_id(&format!("{field}#"))
    );
    let index = shell
        .ui
        .find(&scroll)
        .expect("mixed collections remain expandable");
    shell.ui.scroll_into_view(index).unwrap();
    shell.frame().unwrap();
    let rect = shell.ui.nodes[index].rect;
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    let mut clipboard = Clip::default();
    shell
        .event(
            InputEvent::Wheel {
                position: point,
                delta: [0., 2400.],
            },
            &mut clipboard,
        )
        .unwrap();
    if shell.sync_collections() {
        shell.rebuild(&mut editor).unwrap();
    }
    shell.frame().unwrap();
    let mounted = shell
        .ui
        .nodes
        .iter()
        .filter(|node| {
            node.element
                .id
                .starts_with("nest-edit-MeshRenderer-emissiveMaterials-")
        })
        .count();
    assert!(
        mounted <= 14,
        "only a viewport-sized item pool is mounted: {mounted}"
    );
    let item = "nest-edit-MeshRenderer-emissiveMaterials-100";
    let index = shell
        .ui
        .find(item)
        .expect("item 100 is reachable by actual scrolling");
    shell.ui.scroll_into_view(index).unwrap();
    shell.frame().unwrap();
    shell.click(item, &mut clipboard).unwrap();
    shell
        .event(InputEvent::Edit(EditEvent::SelectAll), &mut clipboard)
        .unwrap();
    shell
        .event(
            InputEvent::Edit(EditEvent::Insert("changed through UI".into())),
            &mut clipboard,
        )
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.commit_fields(&mut editor).unwrap();
    assert_eq!(
        record(&editor, "e_collect01")["MeshRenderer"]["emissiveMaterials"][100],
        "changed through UI"
    );
    assert_eq!(
        record(&editor, "e_collect02")["MeshRenderer"]["emissiveMaterials"],
        json!(["other0", "other1"])
    );
}

fn drag_row(
    editor: &mut Editor,
    shell: &mut Shell,
    source_prefix: &str,
    target_prefix: &str,
    after: bool,
) {
    use ara_ui::input::InputEvent;
    let source = shell
        .ui
        .nodes
        .iter()
        .find(|node| node.element.id.starts_with(source_prefix))
        .expect("source grip mounted")
        .element
        .id
        .clone();
    let source_index = shell.ui.find(&source).unwrap();
    shell.ui.scroll_into_view(source_index).unwrap();
    shell.frame().unwrap();
    let rect = shell.ui.nodes[source_index].rect;
    let start = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    let target = shell
        .ui
        .nodes
        .iter()
        .find(|node| node.element.id.starts_with(target_prefix))
        .expect("target row mounted")
        .rect;
    let end = [
        target.x + target.w * 0.6,
        target.y + target.h * if after { 0.9 } else { 0.1 },
    ];
    let mut clipboard = Clip::default();
    shell.pointer = start;
    shell
        .event(InputEvent::PointerDown(start), &mut clipboard)
        .unwrap();
    shell.begin_drag();
    assert!(
        shell.content_pending(),
        "a collection grip starts the shared drag framework"
    );
    shell.pointer = end;
    shell
        .event(InputEvent::PointerMove(end), &mut clipboard)
        .unwrap();
    shell.content_drag_over(editor, 0.016).unwrap();
    assert_eq!(
        shell.content_drag_report()["feedback"]["valid"],
        true,
        "{}",
        shell.content_drag_report()
    );
    assert!(shell.drop_at(editor).unwrap());
    shell
        .event(InputEvent::PointerUp(end), &mut clipboard)
        .unwrap();
    shell.dispatch(editor).unwrap();
    shell.rebuild(editor).unwrap();
}

#[test]
fn actual_collection_grips_reorder_positional_and_keyed_items_through_shared_drag() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action("editor.select", json!({"entities":["e_collect01"]}))
        .unwrap();
    editor
        .action("editor.view", json!({"maximized":"inspector"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1100., 1100.]).unwrap();
    shell.offscreen = true;
    drag_row(
        &mut editor,
        &mut shell,
        "collection-grip-MeshRenderer-emissiveMaterials-1",
        "nest-MeshRenderer-emissiveMaterials-3",
        true,
    );
    let items = record(&editor, "e_collect01")["MeshRenderer"]["emissiveMaterials"].clone();
    assert_eq!(items[1], "item2");
    assert_eq!(items[3], "item1");
    drag_row(
        &mut editor,
        &mut shell,
        "collection-grip-Tilemap-layers-l-two-",
        "nest-Tilemap-layers-l-one-",
        false,
    );
    let layers = record(&editor, "e_collect01")["Tilemap"]["layers"].clone();
    assert_eq!(layers[0]["id"], "l_two");
    assert_eq!(layers[0]["order"], 2);
    assert_eq!(layers[1]["id"], "l_one");
    assert_eq!(layers[1]["order"], 1);
}
