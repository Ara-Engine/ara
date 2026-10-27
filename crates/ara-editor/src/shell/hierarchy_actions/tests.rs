//! Model/authority coverage for grouping and presentation target sets; no native window.
#![allow(clippy::unwrap_used)]
use super::*;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn open() -> Editor {
    let root = crate::test_target()
        .join("ux-hierarchy-actions")
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
            json!({"name":"Hierarchy actions","main":"levels/main.scene"}),
        ),
        (
            "levels/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_groups01","records":{}}),
        ),
        (
            "prefabs/group.prefab",
            json!({"$schema":"ara://schema/prefab/1","id":"pf_groups01","records":{
                "e_root0001":{"name":"Linked root","Transform":{"position":[1,2,3]}},
                "e_inner001":{"name":"Linked child","parent":"e_root0001","Transform":{"position":[3,2,1]}}
            }}),
        ),
    ] {
        std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
    }
    Editor::open_with(&root, Some(root.join(".ara/test-settings.json"))).unwrap()
}

fn entity(id: &str, parent: Option<&str>, transform: Option<Value>) -> Value {
    let mut call = json!({"cmd":"entity.create","args":{"id":id,"parent":parent,"name":id}});
    if let Some(transform) = transform {
        call["args"]["components"] = json!({"Transform":transform});
    }
    call
}

fn resolved(editor: &Editor) -> Resolved {
    crate::world::resolve(editor.host.store(), &editor.state.doc).unwrap()
}

fn children(resolved: &Resolved, parent: Option<&str>) -> Vec<String> {
    let mut children: Vec<_> = resolved
        .iter()
        .filter(|(_, e)| e.parent.as_deref() == parent)
        .collect();
    children.sort_by(|(a, ea), (b, eb)| ea.order.cmp(&eb.order).then(a.cmp(b)));
    children.into_iter().map(|(id, _)| id.clone()).collect()
}

fn selection(editor: &mut Editor, ids: &[&str]) {
    editor
        .action("editor.select", json!({"entities":ids}))
        .unwrap();
}

fn same_world(before: &Resolved, after: &Resolved) {
    for id in before.keys() {
        let before = crate::world::world_transform(before, id).unwrap();
        let after = crate::world::world_transform(after, id).unwrap();
        assert!(
            before
                .to_cols_array()
                .into_iter()
                .zip(after.to_cols_array())
                .all(|(a, b)| (a - b).abs() < 0.0001 * a.abs().max(1.)),
            "world pose changed for {id}"
        );
    }
}

#[test]
fn group_preserves_top_level_root_order_exact_locals_and_one_undo_redo() {
    let mut editor = open();
    editor
        .apply(
            vec![
                entity(
                    "e_parent01",
                    None,
                    Some(json!({"position":[4,2,8],"rotation":[0,30,0],"scale":[0,2,3]})),
                ),
                entity("e_before01", Some("e_parent01"), Some(json!({}))),
                entity(
                    "e_first001",
                    Some("e_parent01"),
                    Some(json!({"position":[3,4,5],"rotation":[10,20,30]})),
                ),
                entity("e_between1", Some("e_parent01"), Some(json!({}))),
                entity("e_second01", Some("e_parent01"), None),
                entity(
                    "e_child001",
                    Some("e_first001"),
                    Some(json!({"position":[7,8,9]})),
                ),
            ],
            "Create grouped siblings",
            None,
        )
        .unwrap();
    selection(&mut editor, &["e_second01", "e_child001", "e_first001"]);
    let before = resolved(&editor);
    let revision = editor.host.store().revision();
    let selected = editor.state.selection.clone();
    let id = group(&mut editor, &before, &selected).unwrap();
    let after = resolved(&editor);
    assert_eq!(
        editor.host.store().revision(),
        revision + 1,
        "one content transaction"
    );
    assert_eq!(
        children(&after, Some("e_parent01")),
        ["e_before01", id.as_str(), "e_between1"]
    );
    assert_eq!(children(&after, Some(&id)), ["e_first001", "e_second01"]);
    assert_eq!(after["e_child001"].parent.as_deref(), Some("e_first001"));
    assert_eq!(
        after["e_first001"].record["Transform"],
        before["e_first001"].record["Transform"]
    );
    assert!(
        !after["e_second01"].record.contains_key("Transform"),
        "identity grouping preserves absent components"
    );
    same_world(&before, &after);
    assert_eq!(editor.state.selection.as_slice(), std::slice::from_ref(&id));
    assert!(editor.state.expanded.contains(&id));
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(resolved(&editor), before);
    editor.action("history.redo", json!({})).unwrap();
    assert_eq!(resolved(&editor), after);
}

#[test]
fn cross_parent_group_uses_nearest_shared_ancestor_and_preserves_descendant_world_poses() {
    let mut editor = open();
    editor
        .apply(
            vec![
                entity(
                    "e_common01",
                    None,
                    Some(json!({"position":[5,7,9],"rotation":[0,20,0],"scale":[2,2,2]})),
                ),
                entity(
                    "e_branch01",
                    Some("e_common01"),
                    Some(json!({"position":[3,1,2],"rotation":[0,35,0],"scale":[3,3,3]})),
                ),
                entity(
                    "e_branch02",
                    Some("e_common01"),
                    Some(json!({"position":[-2,3,4],"rotation":[0,-10,0],"scale":[0.5,0.5,0.5]})),
                ),
                entity(
                    "e_source01",
                    Some("e_branch01"),
                    Some(json!({"position":[1,2,3],"rotation":[15,0,0]})),
                ),
                entity("e_source02", Some("e_branch02"), None),
                entity(
                    "e_child001",
                    Some("e_source01"),
                    Some(json!({"position":[3,2,1]})),
                ),
            ],
            "Create separate branches",
            None,
        )
        .unwrap();
    let before = resolved(&editor);
    let id = group(
        &mut editor,
        &before,
        &[
            "e_source02".into(),
            "e_child001".into(),
            "e_source01".into(),
        ],
    )
    .unwrap();
    let after = resolved(&editor);
    assert_eq!(after[&id].parent.as_deref(), Some("e_common01"));
    assert_eq!(
        children(&after, Some("e_common01")),
        [id.as_str(), "e_branch01", "e_branch02"]
    );
    assert_eq!(children(&after, Some(&id)), ["e_source01", "e_source02"]);
    assert!(
        after["e_source02"].record.contains_key("Transform"),
        "cross-parent transformless entities gain the preserved pose"
    );
    same_world(&before, &after);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(resolved(&editor), before);
}

#[test]
fn invalid_group_inputs_never_create_a_container_or_partial_reparents() {
    let mut editor = open();
    editor
        .apply(
            vec![
                entity("e_parent01", None, Some(json!({"scale":[2,1,1]}))),
                entity(
                    "e_child001",
                    Some("e_parent01"),
                    Some(json!({"rotation":[0,0,45]})),
                ),
                entity("e_other001", None, Some(json!({}))),
            ],
            "Create rejected grouping fixture",
            None,
        )
        .unwrap();
    let before = resolved(&editor);
    let revision = editor.host.store().revision();
    assert!(
        group(
            &mut editor,
            &before,
            &["e_child001".into(), "e_other001".into()]
        )
        .unwrap_err()
        .to_string()
        .contains("shear")
    );
    assert!(group(&mut editor, &before, &["e_deleted1".into()]).is_err());
    assert!(group(&mut editor, &before, &[]).is_err());
    editor
        .action("editor.lock", json!({"entity":"e_other001","enabled":true}))
        .unwrap();
    assert!(group(&mut editor, &before, &["e_other001".into()]).is_err());
    editor
        .action("editor.lock", json!({"entity":"e_parent01","enabled":true}))
        .unwrap();
    assert!(
        group(&mut editor, &before, &["e_child001".into()])
            .unwrap_err()
            .to_string()
            .contains("destination parent")
    );
    let mut cycle = before.clone();
    cycle.get_mut("e_parent01").unwrap().parent = Some("e_child001".into());
    assert!(
        model::group_commands(&cycle, &["e_child001".into()], &BTreeSet::new())
            .unwrap_err()
            .contains("cycle")
    );
    let mut missing = before.clone();
    missing.get_mut("e_child001").unwrap().parent = Some("e_deleted1".into());
    assert!(model::group_commands(&missing, &["e_child001".into()], &BTreeSet::new()).is_err());
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(resolved(&editor), before);
}

#[test]
fn selected_prefab_child_travels_with_its_selected_instance_without_flattening() {
    let mut editor = open();
    editor
        .apply(
            vec![json!({"cmd":"prefab.instantiate","args":{
                "id":"e_instance1","prefab":"@prefabs/group.prefab#pf_groups01"
            }})],
            "Create linked grouping fixture",
            None,
        )
        .unwrap();
    let before = resolved(&editor);
    let inner = "e_instance1/e_inner001";
    assert!(before.contains_key(inner));
    let selected = BTreeSet::from(["e_instance1", inner]);
    assert_eq!(
        selection_roots(&before, &selected).unwrap(),
        ["e_instance1"]
    );
    assert!(selection_roots(&before, &BTreeSet::from([inner])).is_err());
    let id = group(&mut editor, &before, &[inner.into(), "e_instance1".into()]).unwrap();
    let after = resolved(&editor);
    assert_eq!(children(&after, Some(&id)), ["e_instance1"]);
    assert_eq!(after[inner].parent.as_deref(), Some("e_instance1"));
    assert_eq!(after[inner].source, before[inner].source);
    same_world(&before, &after);
    let document = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .unwrap();
    assert_eq!(
        document.records().len(),
        2,
        "the group and linked instance are the only authored records"
    );
    assert_eq!(
        document.record("e_instance1").unwrap()["prefab"],
        "@prefabs/group.prefab#pf_groups01"
    );
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(resolved(&editor), before);
}

#[test]
fn hide_lock_distinguish_selected_set_descendants_and_context_clicked_object() {
    let mut editor = open();
    editor
        .apply(
            vec![
                entity("e_parent01", None, None),
                entity("e_child001", Some("e_parent01"), None),
                entity("e_other001", None, None),
            ],
            "Create flag targets",
            None,
        )
        .unwrap();
    selection(&mut editor, &["e_parent01", "e_other001"]);
    let all = resolved(&editor);
    let revision = editor.host.store().revision();
    editor
        .action(
            "editor.visibility",
            json!({"entity":"e_other001","enabled":true}),
        )
        .unwrap();
    set_view_flag(&mut editor, &all, &Target::None, false, None, false).unwrap();
    assert_eq!(
        editor.state.hidden,
        BTreeSet::from(["e_parent01".into(), "e_other001".into()])
    );
    set_view_flag(
        &mut editor,
        &all,
        &Target::Entity("e_parent01".into()),
        false,
        None,
        false,
    )
    .unwrap();
    assert!(
        editor.state.hidden.is_empty(),
        "clicking a selected row acts on the entire selection"
    );
    set_view_flag(
        &mut editor,
        &all,
        &Target::Entity("e_child001".into()),
        false,
        Some(true),
        true,
    )
    .unwrap();
    assert_eq!(
        editor.state.locked,
        BTreeSet::from(["e_child001".into()]),
        "an unselected context row does not affect the selection"
    );
    set_view_flag(&mut editor, &all, &Target::None, true, Some(true), true).unwrap();
    assert_eq!(
        editor.state.locked,
        BTreeSet::from([
            "e_parent01".into(),
            "e_child001".into(),
            "e_other001".into()
        ])
    );
    set_view_flag(&mut editor, &all, &Target::None, true, Some(false), true).unwrap();
    assert!(editor.state.locked.is_empty());
    assert_eq!(editor.state.selection, ["e_parent01", "e_other001"]);
    assert_eq!(
        editor.host.store().revision(),
        revision,
        "presentation flags do not enter content undo"
    );
    assert_eq!(resolved(&editor), all);
}

#[test]
fn a_stale_target_prevents_every_presentation_change() {
    let mut editor = open();
    editor
        .apply(
            vec![entity("e_valid001", None, None)],
            "Create live target",
            None,
        )
        .unwrap();
    selection(&mut editor, &["e_valid001"]);
    let mut all = resolved(&editor);
    let selected = vec!["e_valid001".into(), "e_deleted1".into()];
    assert!(model::view_targets(&all, &selected, true).is_err());
    all.get_mut("e_valid001").unwrap().parent = Some("e_deleted1".into());
    assert!(set_view_flag(&mut editor, &all, &Target::None, true, Some(true), false).is_err());
    assert!(editor.state.hidden.is_empty());
}
