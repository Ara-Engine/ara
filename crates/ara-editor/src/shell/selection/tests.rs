use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Project(PathBuf);
impl Project {
    fn new() -> ara_ui::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let root = target.join(format!(
            "selection-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels"))?;
        let records: serde_json::Map<String, Value> = (0..1_000)
            .map(|i| {
                (
                    format!("e_select{i:04}"),
                    json!({"name":format!("Entity {i:04}"),"Transform":{}}),
                )
            })
            .collect();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Selection","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_select00","records":records}),
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
            )?;
        }
        Ok(Self(root))
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Clip;
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}

#[test]
fn real_shift_end_selects_unmounted_tree_rows_and_escape_restores_anchor_focus()
-> ara_ui::Result<()> {
    let project = Project::new()?;
    let mut editor = Editor::open(&project.0)?;
    let mut shell = Shell::new(&mut editor, [1100., 700.])?;
    let initial_pool = shell
        .hierarchy_view
        .as_ref()
        .expect("hierarchy pool")
        .visible_keys()
        .count();
    assert!(initial_pool < 100, "fixture must exercise unmounted rows");
    shell.click_entity("e_select0005", &mut Clip)?;
    shell.dispatch(&mut editor)?;
    shell.key(&mut editor, 225, true, &mut Clip)?;
    shell.key(&mut editor, 77, true, &mut Clip)?;
    shell.key(&mut editor, 225, false, &mut Clip)?;
    assert_eq!(editor.state.selection.len(), 995);
    assert!(editor.state.selection.iter().any(|id| id == "e_select0999"));
    assert!(shell.hierarchy_offset > 0.);
    let target = shell
        .hierarchy_view
        .as_ref()
        .expect("hierarchy pool")
        .identity(crate::world::render_id("e_select0999"));
    assert_eq!(shell.input.focused(), Some(target));
    shell.key(&mut editor, 41, true, &mut Clip)?;
    assert_eq!(editor.state.selection, ["e_select0005"]);
    let anchor = shell
        .hierarchy_view
        .as_ref()
        .expect("hierarchy pool")
        .identity(crate::world::render_id("e_select0005"));
    assert_eq!(shell.input.focused(), Some(anchor));
    Ok(())
}

#[test]
fn filtered_range_uses_display_order_and_repairs_a_deleted_anchor() -> ara_ui::Result<()> {
    let project = Project::new()?;
    let mut editor = Editor::open(&project.0)?;
    let mut shell = Shell::new(&mut editor, [1100., 700.])?;
    shell.select_hierarchy_item(&mut editor, "e_select0002")?;
    editor.action(
        "editor.search",
        json!({"panel":"hierarchy","query":"Entity 000"}),
    )?;
    shell.rebuild(&mut editor)?;
    shell.modifiers[0] = true;
    shell.select_hierarchy_item(&mut editor, "e_select0008")?;
    assert_eq!(editor.state.selection.len(), 7);
    editor.apply(
        vec![json!({"cmd":"entity.delete","args":{"entities":["e_select0002"]}})],
        "Delete range anchor",
        None,
    )?;
    shell.rebuild(&mut editor)?;
    shell.select_hierarchy_item(&mut editor, "e_select0009")?;
    assert!(!editor.state.selection.iter().any(|id| id == "e_select0002"));
    assert!(editor.state.selection.iter().any(|id| id == "e_select0009"));
    Ok(())
}

#[test]
fn shift_end_includes_name_and_id_query_matches_after_numbered_rows_and_escape_restores_anchor()
-> ara_ui::Result<()> {
    let project = Project::new()?;
    let mut editor = Editor::open(&project.0)?;
    editor.apply(vec![json!({"cmd":"entity.create","args":{"id":"e_selectproof","name":"Entity mesh","components":{"Transform":{}}}})],"Append a real displayed query match",None)?;
    editor.action(
        "editor.search",
        json!({"panel":"hierarchy","query":"Entity select"}),
    )?;
    let mut shell = Shell::new(&mut editor, [1100., 700.])?;
    shell.click_entity("e_select0005", &mut Clip)?;
    shell.dispatch(&mut editor)?;
    let before = shell.hierarchy_range_probe(&editor);
    assert_eq!(before["anchor"], "e_select0005");
    assert_eq!(before["active"], "e_select0005");
    assert_eq!(before["focused"], "e_select0005");
    assert_eq!(
        before["order"].as_array().unwrap().last().unwrap(),
        "e_selectproof"
    );
    assert!(before["mountedRows"].as_u64().unwrap() < 100);
    let expected: std::collections::BTreeSet<_> = (5..1000)
        .map(|index| format!("e_select{index:04}"))
        .chain(["e_selectproof".into()])
        .collect();
    let revision = editor.host.store().revision();
    shell.key(&mut editor, 225, true, &mut Clip)?;
    shell.key(&mut editor, 77, true, &mut Clip)?;
    shell.key(&mut editor, 225, false, &mut Clip)?;
    assert_eq!(
        editor
            .state
            .selection
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert_eq!(
        editor.state.selection.first().map(String::as_str),
        Some("e_selectproof")
    );
    assert!(shell.hierarchy_offset > 0.);
    assert_eq!(
        shell.hierarchy_range_probe(&editor)["focused"],
        "e_selectproof"
    );
    shell.key(&mut editor, 41, true, &mut Clip)?;
    assert_eq!(editor.state.selection, ["e_select0005"]);
    assert_eq!(
        shell.hierarchy_range_probe(&editor)["focused"],
        "e_select0005"
    );
    assert_eq!(editor.host.store().revision(), revision);
    Ok(())
}
