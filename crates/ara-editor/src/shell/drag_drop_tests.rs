//! Real retained-shell gestures against the editor authority, with scratch documents only.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_ui::input::InputEvent;
use std::sync::atomic::{AtomicU64, Ordering};

fn collection_value(editor: &Editor) -> Value {
    json!(
        editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .unwrap()
            .record(IDS[0])
            .unwrap()
    )["MeshRenderer"]["emissiveMaterials"]
        .clone()
}

#[test]
fn held_collection_edge_scroll_reaches_item_one_hundred_and_reorders_once() {
    let (mut editor, mut shell) = open();
    let original: Vec<_> = (0..160).map(|index| format!("item{index}")).collect();
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":IDS[0],"path":"MeshRenderer.emissiveMaterials","value":original}})], "Populate collection", None).unwrap();
    editor
        .action("editor.select", json!({"entities":[IDS[0]]}))
        .unwrap();
    editor
        .action("editor.view", json!({"maximized":"inspector"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    let scroll = collection_adapter::id("MeshRenderer.emissiveMaterials", "");
    let scroll_index = shell.ui.find(&scroll).unwrap();
    shell.ui.scroll_into_view(scroll_index).unwrap();
    shell.frame().unwrap();
    let grip = shell
        .shown_node("collection-grip-MeshRenderer-emissiveMaterials-0")
        .unwrap();
    let start = centre(
        shell.ui.nodes[grip]
            .rect
            .intersection(shell.ui.nodes[grip].clip),
    );
    let viewport = shell.ui.nodes[scroll_index]
        .rect
        .intersection(shell.ui.nodes[scroll_index].clip);
    assert!(viewport.h >= 48.);
    assert!(
        shell
            .shown_node("nest-MeshRenderer-emissiveMaterials-100")
            .is_none()
    );
    let revision = editor.host.store().revision();
    let mut clipboard = Clipboard::default();
    shell.pointer = start;
    shell
        .event(InputEvent::PointerDown(start), &mut clipboard)
        .unwrap();
    shell.begin_drag();
    let edge = [viewport.x + viewport.w * 0.6, viewport.y + viewport.h + 8.];
    shell.pointer = edge;
    shell
        .event(InputEvent::PointerMove(edge), &mut clipboard)
        .unwrap();
    let mut target = None;
    let mut max_pool = 0;
    for _ in 0..120 {
        // No more PointerMove events: the held edge itself must keep scrolling.
        shell.content_drag_over(&editor, 0.1).unwrap();
        shell.dispatch(&mut editor).unwrap();
        shell.frame().unwrap();
        assert_eq!(shell.pointer, edge);
        assert_eq!(editor.host.store().revision(), revision);
        max_pool = max_pool.max(
            shell
                .ui
                .nodes
                .iter()
                .filter(|node| {
                    node.element
                        .id
                        .starts_with("nest-MeshRenderer-emissiveMaterials-")
                        && !node.element.id["nest-MeshRenderer-emissiveMaterials-".len()..]
                            .contains('-')
                })
                .count(),
        );
        if let Some(index) = shell.shown_node("nest-MeshRenderer-emissiveMaterials-100") {
            let row = shell.ui.nodes[index].rect;
            let visible = row.intersection(shell.ui.nodes[index].clip);
            if visible.h >= row.h - 1. {
                target = Some([row.x + row.w * 0.6, row.y + row.h - 1.]);
                break;
            }
        }
    }
    assert!(max_pool <= 14, "bounded collection pool: {max_pool}");
    let target = target.expect("held native-style edge scroll reaches item 100");
    assert!(
        shell
            .ui
            .scroll_position(shell.ui.find(&scroll).unwrap())
            .unwrap()[1]
            > 2000.
    );
    shell.pointer = target;
    shell
        .event(InputEvent::PointerMove(target), &mut clipboard)
        .unwrap();
    shell.content_drag_over(&editor, 0.).unwrap();
    let feedback = shell.content_drag_report();
    assert_eq!(feedback["feedback"]["valid"], true, "{feedback}");
    assert_eq!(feedback["feedback"]["target"]["after"], true);
    assert!(shell.drop_at(&mut editor).unwrap());
    shell
        .event(InputEvent::PointerUp(target), &mut clipboard)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    let mut expected = original.clone();
    let moved = expected.remove(0);
    expected.insert(100, moved);
    assert_eq!(collection_value(&editor), json!(expected));
    assert_eq!(editor.host.store().revision(), revision + 1);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(collection_value(&editor), json!(original));
    editor.action("history.redo", json!({})).unwrap();
    assert_eq!(collection_value(&editor), json!(expected));
}

#[test]
fn collection_edge_scroll_rejects_hidden_and_unrelated_pointer_geometry() {
    let (mut editor, mut shell) = open();
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":IDS[0],"path":"MeshRenderer.emissiveMaterials","value":(0..160).map(|index|format!("item{index}")).collect::<Vec<_>>()}})], "Populate collection", None).unwrap();
    editor
        .action("editor.select", json!({"entities":[IDS[0]]}))
        .unwrap();
    editor
        .action("editor.view", json!({"maximized":"inspector"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    let id = collection_adapter::id("MeshRenderer.emissiveMaterials", "");
    let index = shell.ui.find(&id).unwrap();
    shell.ui.scroll_into_view(index).unwrap();
    shell.frame().unwrap();
    let source = match shell.bindings.get(&stable_id(
        "collection-grip-MeshRenderer-emissiveMaterials-0",
    )) {
        Some(Binding::Collection(action)) => action.clone(),
        _ => panic!("Collection source binding missing"),
    };
    let rect = shell.ui.nodes[index]
        .rect
        .intersection(shell.ui.nodes[index].clip);
    let before = shell.ui.scroll_position(index).unwrap();
    for point in [
        [f32::NAN, rect.y],
        [rect.x + rect.w + 1., rect.y + rect.h + 8.],
        [rect.x + rect.w * 0.5, rect.y + rect.h + 25.],
    ] {
        shell.pointer = point;
        assert!(!shell.scroll_drag_collection(&source, 0.1).unwrap());
        assert_eq!(shell.ui.scroll_position(index).unwrap(), before);
    }
    shell.pointer = [rect.x + rect.w * 0.5, rect.y + rect.h + 8.];
    shell.ui.set_visible(index, false).unwrap();
    assert!(!shell.scroll_drag_collection(&source, 0.1).unwrap());
    assert_eq!(shell.ui.scroll_position(index).unwrap(), before);
}

#[test]
#[ignore = "explicit before/after collection-drag CPU benchmark; writes only target evidence"]
fn benchmark_collection_drag_scroll_adapter() {
    use std::{hint::black_box, time::Instant};
    let (mut editor, mut shell) = open();
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":IDS[0],"path":"MeshRenderer.emissiveMaterials","value":(0..160).map(|index|format!("item{index}")).collect::<Vec<_>>()}})], "Populate collection", None).unwrap();
    editor
        .action("editor.select", json!({"entities":[IDS[0]]}))
        .unwrap();
    editor
        .action("editor.view", json!({"maximized":"inspector"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    let id = collection_adapter::id("MeshRenderer.emissiveMaterials", "");
    let index = shell.ui.find(&id).unwrap();
    shell.ui.scroll_into_view(index).unwrap();
    shell.frame().unwrap();
    let source = match shell.bindings.get(&stable_id(
        "collection-grip-MeshRenderer-emissiveMaterials-0",
    )) {
        Some(Binding::Collection(action)) => action.clone(),
        _ => panic!("Collection source binding missing"),
    };
    shell.pointer = centre(
        shell.ui.nodes[index]
            .rect
            .intersection(shell.ui.nodes[index].clip),
    );
    let previous = |shell: &Shell| {
        // Exact previous fixed scroll-target lookup/math, with zero elapsed time
        // to keep this microbenchmark independent of layout and row rebuilding.
        for id in [
            "hierarchy-rows",
            "asset-rows",
            "asset-grid",
            "asset-folders",
            "inspector-scroll",
        ] {
            if let Some(index) = shell.shown_node(black_box(id)) {
                let node = &shell.ui.nodes[index];
                black_box(kit::auto_scroll(
                    node.rect.intersection(node.clip),
                    shell.pointer,
                    black_box(0.),
                ));
            }
        }
    };
    for _ in 0..1000 {
        previous(&shell);
        black_box(
            shell
                .scroll_drag_collection(&source, black_box(0.))
                .unwrap(),
        );
    }
    let mut old = Vec::new();
    let mut current = Vec::new();
    for round in 0..5 {
        let mut measure = |after: bool| {
            let start = Instant::now();
            for _ in 0..10_000 {
                if after {
                    black_box(
                        shell
                            .scroll_drag_collection(black_box(&source), black_box(0.))
                            .unwrap(),
                    );
                } else {
                    previous(black_box(&shell));
                }
            }
            start.elapsed().as_secs_f64() * 1e9 / 10_000.
        };
        if round % 2 == 0 {
            old.push(measure(false));
            current.push(measure(true));
        } else {
            current.push(measure(true));
            old.push(measure(false));
        }
    }
    let median = |values: &[f64]| {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    let output = crate::test_target().join("editor-drag-benchmarks");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join(format!("collection-scroll-{}.json",std::process::id())),ara_json::to_canonical_string(&json!({
        "baseline":"68208897 fixed outer scroll target lookup/math (test-only reproduction)",
        "current":"source collection viewport lookup/math and outer-scroll suppression",
        "scope":"active collection drag adapter only; dt=0, unchanged retained UI; no I/O/layout in timed loops",
        "moduleBlake3":blake3::hash(include_str!("drag_drop.rs").as_bytes()).to_hex().to_string(),
        "warmup":1000,"rounds":5,"callsPerRound":10_000,"baselineNsPerCall":old,"currentNsPerCall":current,
        "baselineMedianNs":median(&old),"currentMedianNs":median(&current),"debugAssertions":cfg!(debug_assertions),"arch":std::env::consts::ARCH,"os":std::env::consts::OS
    }))).unwrap();
}

static NEXT: AtomicU64 = AtomicU64::new(0);
const IDS: [&str; 5] = [
    "e_draga01",
    "e_dragb01",
    "e_dragc01",
    "e_dragd01",
    "e_drage01",
];
#[derive(Default)]
struct Clipboard(String);
impl ara_ui_text::Clipboard for Clipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}
fn open() -> (Editor, Shell) {
    let root = crate::test_target().join("ux-drag-tests").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("assets")).unwrap();
    for (path, value) in [
        (
            "ara.project.json",
            json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Drag gestures","main":"levels/main.scene"}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_drag0001","records":{}}),
        ),
        (
            "assets/box.prefab",
            json!({"$schema":"ara://schema/prefab/1","id":"pf_drag0001","records":{"e_prefab01":{"name":"Box","Transform":{},"MeshRenderer":{}}}}),
        ),
    ] {
        std::fs::write(
            root.join(if path == "ara.project.json" {
                std::path::PathBuf::from(path)
            } else {
                std::path::PathBuf::from(ara_doc::AUTHORING_ROOT).join(path)
            }),
            ara_json::to_canonical_string(&value),
        )
        .unwrap();
    }
    let mut editor = Editor::open(&root).unwrap();
    editor
        .action("editor.preferences", json!({"reduceMotion":true}))
        .unwrap();
    editor.apply(IDS.iter().enumerate().map(|(n, id)| json!({"cmd":"entity.create","args":{"id":id,"name":format!("Entity {n}"),"components":{"Transform":{},"MeshRenderer":{}}}})).collect(), "Create drag fixtures", None).unwrap();
    let shell = Shell::new(&mut editor, [1440., 1800.]).unwrap();
    (editor, shell)
}
fn row(shell: &Shell, entity: &str) -> Box2 {
    shell
        .bindings
        .iter()
        .find_map(|(id, binding)| {
            if !matches!(binding, Binding::Select(id) if id == entity) {
                return None;
            }
            let i = shell.ui.by_id(*id)?;
            shell.ui.shown(i).then_some(shell.ui.nodes[i].rect)
        })
        .unwrap_or_else(|| panic!("No visible hierarchy row for {entity}"))
}
fn centre(rect: Box2) -> [f32; 2] {
    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]
}
fn begin(shell: &mut Shell, entity: &str, clipboard: &mut Clipboard) {
    shell.pointer = centre(row(shell, entity));
    shell
        .event(InputEvent::PointerDown(shell.pointer), clipboard)
        .unwrap();
    shell.begin_drag();
    assert!(shell.content_pending());
}
fn move_to(shell: &mut Shell, editor: &Editor, pointer: [f32; 2], clipboard: &mut Clipboard) {
    shell.pointer = pointer;
    shell
        .event(InputEvent::PointerMove(pointer), clipboard)
        .unwrap();
    shell.content_drag_over(editor, 0.).unwrap();
    shell.frame().unwrap();
}
fn roots(editor: &Editor) -> Vec<String> {
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    let mut roots: Vec<_> = resolved
        .iter()
        .filter(|(_, e)| e.parent.is_none())
        .collect();
    roots.sort_by(|(a, ea), (b, eb)| ea.order.cmp(&eb.order).then(a.cmp(b)));
    roots.into_iter().map(|(id, _)| id.to_owned()).collect()
}
/// Linked metadata is authored in the scene; resolved entities expose source and components.
fn placed_prefab(editor: &Editor) -> String {
    let document = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .unwrap();
    let instances: Vec<_> = document
        .records()
        .iter()
        .filter(|(_, record)| {
            record.get("prefab").and_then(Value::as_str) == Some("@assets/box.prefab#pf_drag0001")
        })
        .collect();
    assert_eq!(
        instances.len(),
        1,
        "one linked prefab must be authored by the drop"
    );
    let (id, record) = instances[0];
    assert!(
        !record.contains_key("Transform"),
        "instance pose belongs in overrides"
    );
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert_eq!(resolved[id.as_str()].source.0.as_str(), "pf_drag0001");
    assert!(resolved[id.as_str()].record.contains_key("MeshRenderer"));
    id.to_string()
}
fn show_asset(editor: &mut Editor, shell: &mut Shell) {
    editor
        .action("editor.layout.show", json!({"panel":"assets"}))
        .unwrap();
    editor
        .action("editor.workspace", json!({"folder":"assets"}))
        .unwrap();
    shell.browser.insert_path("assets/box.prefab", "prefab");
    shell.rebuild(editor).unwrap();
}
fn begin_asset(shell: &mut Shell, clip: &mut Clipboard) {
    let rect = shell
        .bindings
        .iter()
        .find_map(|(id, binding)| {
            if !matches!(binding, Binding::Asset(asset) if asset.path == format!("{}/assets/box.prefab",ara_doc::AUTHORING_ROOT)) {
                return None;
            }
            let i = shell.ui.by_id(*id)?;
            shell.ui.shown(i).then_some(shell.ui.nodes[i].rect)
        })
        .expect("fixture prefab has a visible asset card");
    shell.pointer = centre(rect);
    shell
        .event(InputEvent::PointerDown(shell.pointer), clip)
        .unwrap();
    shell.begin_drag();
    assert!(shell.content_pending());
}

#[test]
fn multi_row_drop_preserves_visual_order_uses_one_transaction_and_undo_restores_it() {
    let (mut editor, mut shell) = open();
    editor
        .action("editor.select", json!({"entities":[IDS[2], IDS[1]]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let before = editor.host.store().revision();
    let mut clip = Clipboard::default();
    begin(&mut shell, IDS[1], &mut clip);
    let target = row(&shell, IDS[3]);
    move_to(
        &mut shell,
        &editor,
        [target.x + target.w * 0.5, target.y + target.h - 1.],
        &mut clip,
    );
    assert!(shell.content_dragging());
    assert!(shell.ui.display.commands.iter().any(|draw| matches!(draw, Draw::Rect { node, rect, .. } if *node == stable_id("content-drop-preview") && rect.h == 2.)));
    assert!(shell.drop_at(&mut editor).unwrap());
    shell.event(InputEvent::Cancel, &mut clip).unwrap();
    assert_eq!(editor.host.store().revision(), before + 1);
    assert_eq!(roots(&editor), [IDS[0], IDS[3], IDS[1], IDS[2], IDS[4]]);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(roots(&editor), IDS);
}

#[test]
fn a_selected_child_travels_with_its_root_and_cycles_are_refused_before_commit() {
    let (mut editor, mut shell) = open();
    editor.apply(vec![json!({"cmd":"entity.create","args":{"id":"e_child001","parent":IDS[1],"name":"Child"}})], "Add child", None).unwrap();
    editor
        .action(
            "editor.select",
            json!({"entities":[IDS[1],"e_child001",IDS[2]]}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let mut clip = Clipboard::default();
    begin(&mut shell, IDS[1], &mut clip);
    let destination = centre(row(&shell, IDS[3]));
    move_to(&mut shell, &editor, destination, &mut clip);
    shell.drop_at(&mut editor).unwrap();
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert_eq!(resolved[IDS[1]].parent.as_deref(), Some(IDS[3]));
    assert_eq!(resolved[IDS[2]].parent.as_deref(), Some(IDS[3]));
    assert_eq!(resolved["e_child001"].parent.as_deref(), Some(IDS[1]));
    shell.event(InputEvent::Cancel, &mut clip).unwrap();
    editor
        .action("editor.select", json!({"entities":[IDS[3]]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let before = editor.host.store().revision();
    begin(&mut shell, IDS[3], &mut clip);
    let child = centre(row(&shell, "e_child001"));
    move_to(&mut shell, &editor, child, &mut clip);
    assert!(
        matches!(&shell.drag, Some(Drag::Content(drag)) if matches!(drag.session.feedback(), Some(Feedback::Reject { reason, .. }) if reason.contains("descendants")))
    );
    assert!(shell.drop_at(&mut editor).unwrap());
    assert_eq!(editor.host.store().revision(), before);
}

#[test]
fn clicks_escape_deleted_sources_and_locked_targets_never_mutate_the_document() {
    let (mut editor, mut shell) = open();
    let mut clip = Clipboard::default();
    begin(&mut shell, IDS[1], &mut clip);
    let before = editor.host.store().revision();
    shell.pointer[0] += 2.;
    assert!(!shell.drop_at(&mut editor).unwrap());
    assert_eq!(editor.host.store().revision(), before);
    shell.event(InputEvent::Cancel, &mut clip).unwrap();
    begin(&mut shell, IDS[1], &mut clip);
    let destination = centre(row(&shell, IDS[3]));
    move_to(&mut shell, &editor, destination, &mut clip);
    shell.key(&mut editor, 41, true, &mut clip).unwrap();
    assert!(!shell.content_pending());
    assert_eq!(editor.host.store().revision(), before);
    begin(&mut shell, IDS[1], &mut clip);
    editor
        .apply(
            vec![json!({"cmd":"entity.delete","args":{"entities":[IDS[1]]}})],
            "Delete during drag",
            None,
        )
        .unwrap();
    let deleted = editor.host.store().revision();
    move_to(&mut shell, &editor, destination, &mut clip);
    assert!(shell.drop_at(&mut editor).unwrap());
    assert_eq!(editor.host.store().revision(), deleted);
    shell.event(InputEvent::Cancel, &mut clip).unwrap();
    begin(&mut shell, IDS[2], &mut clip);
    editor.state.locked.insert(IDS[3].into());
    let destination = centre(row(&shell, IDS[3]));
    move_to(&mut shell, &editor, destination, &mut clip);
    assert!(shell.drop_at(&mut editor).unwrap());
    assert_eq!(editor.host.store().revision(), deleted);
}

#[test]
fn entity_drop_assigns_a_reference_without_changing_selection() {
    let (mut editor, mut shell) = open();
    editor.apply(vec![json!({"cmd":"entity.create","args":{"id":"e_camera01","name":"Camera","components":{"Transform":{},"Camera":{},"FollowCamera":{}}}})], "Create camera", None).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_camera01"]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let mut clip = Clipboard::default();
    begin(&mut shell, IDS[1], &mut clip);
    let i = shell.shown_node("edit-FollowCamera-target").unwrap();
    let target = centre(shell.ui.nodes[i].rect);
    move_to(&mut shell, &editor, target, &mut clip);
    shell.drop_at(&mut editor).unwrap();
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert_eq!(
        resolved["e_camera01"].record["FollowCamera"]["target"],
        IDS[1]
    );
    assert_eq!(editor.state.selection, ["e_camera01"]);
}

#[test]
fn reparent_preserves_world_pose_under_a_rotated_scaled_parent_in_one_undo() {
    let (mut editor, mut shell) = open();
    editor.apply(vec![
        json!({"cmd":"transform.set","args":{"entities":[IDS[0]],"position":[10.,2.,3.],"rotation":[0.,90.,0.],"scale":[2.,2.,2.]}}),
        json!({"cmd":"transform.set","args":{"entities":[IDS[1]],"position":[1.,2.,3.],"rotation":[10.,20.,30.]}}),
    ], "Prepare transformed parent", None).unwrap();
    editor
        .action("editor.select", json!({"entities":[IDS[1]]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    let before = crate::world::world_transform(&resolved, IDS[1]).unwrap();
    let revision = editor.host.store().revision();
    let mut clip = Clipboard::default();
    begin(&mut shell, IDS[1], &mut clip);
    let target = centre(row(&shell, IDS[0]));
    move_to(&mut shell, &editor, target, &mut clip);
    shell.drop_at(&mut editor).unwrap();
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    let after = crate::world::world_transform(&resolved, IDS[1]).unwrap();
    assert!(
        before
            .to_cols_array()
            .into_iter()
            .zip(after.to_cols_array())
            .all(|(a, b)| (a - b).abs() < 0.0001)
    );
    assert_eq!(editor.host.store().revision(), revision + 1);
    editor.action("history.undo", json!({})).unwrap();
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert!(resolved[IDS[1]].parent.is_none());
    assert_eq!(
        crate::world::world_transform(&resolved, IDS[1]).unwrap(),
        before
    );
}

#[test]
fn component_header_drag_changes_only_inspector_presentation_order() {
    let (mut editor, mut shell) = open();
    editor
        .action("editor.select", json!({"entities":[IDS[1]]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let before = editor.host.store().revision();
    let mut clip = Clipboard::default();
    let source = shell.shown_node("collapse-component-MeshRenderer").unwrap();
    shell.pointer = centre(shell.ui.nodes[source].rect);
    shell
        .event(InputEvent::PointerDown(shell.pointer), &mut clip)
        .unwrap();
    shell.begin_drag();
    assert!(shell.content_pending());
    let target = shell.ui.nodes[shell.shown_node("component-Transform").unwrap()].rect;
    move_to(
        &mut shell,
        &editor,
        [target.x + 60., target.y + 1.],
        &mut clip,
    );
    shell.drop_at(&mut editor).unwrap();
    assert_eq!(
        shell.inspector_sections.position("MeshRenderer"),
        Some((0, 2))
    );
    assert_eq!(editor.host.store().revision(), before);
}

#[test]
fn prefab_asset_drop_uses_the_indicated_hierarchy_slot_and_invalid_reference_is_graceful() {
    let (mut editor, mut shell) = open();
    editor
        .action("editor.select", json!({"entities":[IDS[1]]}))
        .unwrap();
    show_asset(&mut editor, &mut shell);
    let revision = editor.host.store().revision();
    let mut clip = Clipboard::default();
    begin_asset(&mut shell, &mut clip);
    let target = row(&shell, IDS[3]);
    move_to(
        &mut shell,
        &editor,
        [target.x + 80., target.y + 1.],
        &mut clip,
    );
    let preview = shell.content_drag_report();
    assert_eq!(preview["feedback"]["valid"], true);
    assert_eq!(preview["feedback"]["target"]["kind"], "hierarchy");
    assert_eq!(preview["feedback"]["target"]["placement"], "Before");
    assert!(shell.drop_at(&mut editor).unwrap());
    assert_eq!(editor.host.store().revision(), revision + 1);
    let placed = placed_prefab(&editor);
    assert_eq!(
        roots(&editor),
        [IDS[0], IDS[1], IDS[2], placed.as_str(), IDS[3], IDS[4]]
    );
    shell.event(InputEvent::Cancel, &mut clip).unwrap();
    let before = editor.host.store().revision();
    begin_asset(&mut shell, &mut clip);
    let target =
        centre(shell.ui.nodes[shell.shown_node("asset-field-MeshRenderer-model").unwrap()].rect);
    move_to(&mut shell, &editor, target, &mut clip);
    assert!(shell.drop_at(&mut editor).unwrap());
    assert_eq!(editor.host.store().revision(), before);
    assert!(
        shell
            .status_note
            .as_ref()
            .is_some_and(|note| note.contains("not box.prefab"))
    );
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(roots(&editor), IDS, "one undo removes the placed instance");
}

#[test]
fn surface_snap_survives_release_revalidation_and_reference_drop_crosses_surfaces() {
    let (mut editor, mut shell) = open();
    show_asset(&mut editor, &mut shell);
    let revision = editor.host.store().revision();
    let mut clip = Clipboard::default();
    begin_asset(&mut shell, &mut clip);
    let viewport = shell.viewport().unwrap();
    move_to(&mut shell, &editor, centre(viewport), &mut clip);
    shell.content_surface_hit([1., 2., 3.]);
    assert_eq!(
        shell.content_drag_report()["feedback"]["target"]["position"],
        json!([1., 2., 3.])
    );
    assert!(shell.drop_at(&mut editor).unwrap());
    assert_eq!(editor.host.store().revision(), revision + 1);
    let placed = placed_prefab(&editor);
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert_eq!(
        serde_json::from_value::<[f32; 3]>(
            resolved[&placed].record["Transform"]["position"].clone()
        )
        .unwrap(),
        [1., 2., 3.]
    );
    shell.event(InputEvent::Cancel, &mut clip).unwrap();
    editor.apply(vec![json!({"cmd":"entity.create","args":{"id":"e_camera01","name":"Camera","components":{"Transform":{},"Camera":{},"FollowCamera":{}}}})], "Create camera", None).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_camera01"]}))
        .unwrap();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"inspector","slot":"float","rect":[800,100,500,1600]}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    begin(&mut shell, IDS[1], &mut clip);
    shell.content_drag_pointer([shell.pointer[0] + 200., shell.pointer[1] + 100.]);
    let surface = editor.state.floating.iter().find(|float| matches!(&float.root, ara_ui::docking::DockNode::Area { tabs, .. } if tabs.iter().any(|tab| tab == "inspector"))).map(|float| float.id.clone()).unwrap();
    assert!(shell.enter(&surface));
    let field = shell.shown_node("edit-FollowCamera-target").unwrap();
    shell.pointer = centre(shell.ui.nodes[field].rect);
    shell.content_drag_pointer([shell.pointer[0] + 800., shell.pointer[1] + 100.]);
    shell.content_drag_over(&editor, 0.).unwrap();
    assert!(shell.content_dragging());
    shell.drop_at(&mut editor).unwrap();
    shell.leave();
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert_eq!(
        resolved["e_camera01"].record["FollowCamera"]["target"],
        IDS[1]
    );
}
