//! Mounted data enums use native Select; selection lowers against current state.
#![allow(clippy::unwrap_used)]
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = crate::test_target()
            .join("data-choice-controls")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Data choices","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_choice001","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
    fn open(&self) -> (Editor, Shell) {
        let mut editor =
            Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap();
        let shell = Shell::new(&mut editor, [1200., 800.]).unwrap();
        (editor, shell)
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
fn focus(shell: &mut Shell, id: &str) {
    let index = shell.shown_node(id).expect("mounted data Select");
    shell.ui.scroll_into_view(index).unwrap();
    shell.frame().unwrap();
    assert_eq!(shell.ui.nodes[index].element.tag, "dropdown");
    shell
        .input
        .focus(&mut shell.ui, ara_ui::source::stable_id(id))
        .unwrap();
}

#[test]
fn native_asset_type_select_preserves_search_terms_and_remains_reachable_in_narrow_docks() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor
        .action("editor.layout.show", json!({"panel":"assets"}))
        .unwrap();
    editor
        .action(
            "editor.search",
            json!({"panel":"assets","query":"brick type:mesh hero"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let revision = editor.host.store().revision();
    focus(&mut shell, "asset-type-select");
    shell.key(&mut editor, 40, true, &mut Clip).unwrap();
    shell.key(&mut editor, 77, true, &mut Clip).unwrap();
    shell.key(&mut editor, 40, true, &mut Clip).unwrap();
    assert_eq!(
        editor.state.searches.get("assets").map(String::as_str),
        Some("brick hero type:vfx")
    );
    focus(&mut shell, "asset-type-select");
    shell.key(&mut editor, 40, true, &mut Clip).unwrap();
    shell.key(&mut editor, 74, true, &mut Clip).unwrap();
    shell.key(&mut editor, 40, true, &mut Clip).unwrap();
    assert_eq!(
        editor.state.searches.get("assets").map(String::as_str),
        Some("brick hero")
    );
    shell.ui.resize([640., 480.]).unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    let index = shell
        .shown_node("asset-type-select")
        .expect("narrow dock retains its data filter");
    let visible = shell.ui.nodes[index]
        .rect
        .intersection(shell.ui.nodes[index].clip);
    assert!(visible.w > 40. && visible.h > 15.);
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(with_types("hero type:font type:mesh", &[]), "hero");
    assert_eq!(selected("hero type:font type:mesh"), ["font", "mesh"]);
}

#[test]
fn populated_animation_pose_select_keeps_background_controller_edits_and_one_undo() {
    use ara_anim::graph::NodeKind;
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let created = editor
        .action(
            "editor.resource.create",
            json!({"path":"choices/controller.animation","domain":"animation"}),
        )
        .unwrap();
    let doc = created["doc"].as_str().unwrap().to_owned();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"animation","slot":"view"}),
        )
        .unwrap();
    editor
        .action("editor.focus", json!({"panel":"animation"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.edit_animation(&mut editor, "state").unwrap();
    shell.rebuild(&mut editor).unwrap();
    let node = shell
        .active_graph()
        .unwrap()
        .canvas
        .graph()
        .nodes
        .keys()
        .next()
        .unwrap()
        .clone();
    editor
        .action("editor.canvas", json!({"selection":[node]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    focus(&mut shell, "animation-operation");
    let elements: Vec<_> = shell
        .ui
        .nodes
        .iter()
        .map(|node| node.element.clone())
        .collect();
    assert!(ara_ui::controls::audit(&elements).is_empty());
    shell.key(&mut editor, 40, true, &mut Clip).unwrap();
    let popup_focus = shell.input.focused();
    assert!(shell.choice_open());
    let (record, mut latest) =
        crate::authoring::animation::controller(editor.host.store().workspace().doc(&doc).unwrap())
            .unwrap();
    latest.fixed_step = 1600;
    editor.apply_to(&doc,vec![json!({"cmd":"editor.animation.controller.set","args":{"record":record,"controller":latest}})],"Background controller timing").unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(shell.input.focused(), popup_focus);
    let revision = editor.host.store().revision();
    shell.key(&mut editor, 81, true, &mut Clip).unwrap();
    shell.key(&mut editor, 40, true, &mut Clip).unwrap();
    let (_, controller) =
        crate::authoring::animation::controller(editor.host.store().workspace().doc(&doc).unwrap())
            .unwrap();
    assert_eq!(controller.fixed_step, 1600);
    assert_eq!(
        controller.nodes.values().next().unwrap().kind,
        NodeKind::Blend1d
    );
    let control = shell.ui.nodes[shell.shown_node("animation-operation").unwrap()].rect;
    let row = shell.ui.nodes[shell.shown_node("animation-operation-row").unwrap()].rect;
    assert!(
        control.y >= row.y - 0.5 && control.y + control.h <= row.y + row.h + 0.5,
        "wrapped Select must fit its property row"
    );
    assert_eq!(editor.host.store().revision(), revision + 1);
    editor.action("history.undo", json!({})).unwrap();
    let (_, undone) =
        crate::authoring::animation::controller(editor.host.store().workspace().doc(&doc).unwrap())
            .unwrap();
    assert_eq!(
        serde_json::to_value(undone).unwrap(),
        serde_json::to_value(latest).unwrap()
    );
}
