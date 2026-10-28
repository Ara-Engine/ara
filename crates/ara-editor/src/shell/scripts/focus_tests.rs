//! Inline rename intent survives a real modal/Select stack without taking its focus.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_ui::{input::InputEvent, source::stable_id};
use std::sync::atomic::{AtomicU64, Ordering};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("deferred-rename-focus")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&json!({
            "layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Deferred focus","main":"levels/main.scene"
        }))).unwrap();
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
            ara_json::to_canonical_string(&json!({
                "$schema":"ara://schema/scene/1","id":"sc_rename_focus","records":{}
            })),
        )
        .unwrap();
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open_decision(shell: &mut Shell, editor: &mut Editor) {
    shell.dialog = Some(dialog::Dialog::ScriptModules {
        paths: vec!["Game module".into(), "Editor module".into()],
        selected: Some("Game module".into()),
        pending: false,
        error: None,
    });
    shell.rebuild(editor).unwrap();
    shell.frame().unwrap();
}
fn click(shell: &mut Shell, editor: &mut Editor, id: &str) {
    let index = shell.shown_node(id).unwrap();
    let node = &shell.ui.nodes[index];
    let rect = node.rect.intersection(node.clip);
    assert!(
        rect.w > 0. && rect.h > 0.,
        "{id}: {:?}/{:?}",
        node.rect,
        node.clip
    );
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    assert_eq!(
        shell
            .ui
            .hit_test(point[0], point[1])
            .map(|i| shell.ui.nodes[i].element.id.as_str()),
        Some(id)
    );
    shell
        .event(InputEvent::PointerDown(point), &mut NoClipboard)
        .unwrap();
    shell
        .event(InputEvent::PointerUp(point), &mut NoClipboard)
        .unwrap();
    shell.dispatch(editor).unwrap();
    shell.frame().unwrap();
}

#[test]
fn script_rename_rebuild_defers_to_modal_and_select_then_restores_after_hide_move_and_resize() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"assets","slot":"view"}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [900., 650.]).unwrap();
    shell.offscreen = true;
    // This is the production post-create presentation state, without launching a
    // compiler or writing source as part of the measured restoration interactions.
    shell.browser.insert_path("content/NewSystem.cs", "cs");
    shell.asset_rename = Some(Rename {
        path: "content/NewSystem.cs".into(),
        name: "NewSystem".into(),
        attach_to: vec![],
    });
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    let rename = stable_id("asset-rename-value");
    assert_eq!(shell.input.focused(), Some(rename));
    let revision = editor.host.store().revision();
    open_decision(&mut shell, &mut editor);
    click(&mut shell, &mut editor, "script-module-choice");
    let search = stable_id("choice-script-module-choice-search");
    assert_eq!(shell.input.focused(), Some(search));
    assert!(
        shell.input.focus(&mut shell.ui, rename).is_err(),
        "explicit bad focus remains rejected"
    );
    editor
        .action("editor.layout.close", json!({"panel":"assets"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(shell.input.focused(), Some(search));
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"assets","slot":"bottom"}),
        )
        .unwrap();
    shell.ui.resize([320., 450.]).unwrap();
    for _ in 0..3 {
        editor.tick();
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert_eq!(shell.input.focused(), Some(search));
        assert_eq!(shell.asset_rename.as_ref().unwrap().name, "NewSystem");
    }
    shell.key(&mut editor, 41, true, &mut NoClipboard).unwrap();
    shell.frame().unwrap();
    assert!(
        shell.dialog.is_some(),
        "first Escape closes only the nested Select"
    );
    assert_eq!(
        shell.input.focused(),
        Some(stable_id("script-module-choice"))
    );
    shell.key(&mut editor, 41, true, &mut NoClipboard).unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert!(shell.dialog.is_none());
    assert_eq!(
        shell.input.focused(),
        Some(rename),
        "deferred rename remains eligible after the decision closes"
    );
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn asset_file_rename_requested_behind_a_dialog_is_focused_only_after_the_decision_closes() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"assets","slot":"view"}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [900., 650.]).unwrap();
    shell.offscreen = true;
    let asset = shell
        .browser
        .items
        .iter()
        .find(|asset| asset.kind == "scene")
        .unwrap()
        .clone();
    let folder = asset.path.rsplit_once('/').map_or("", |(folder, _)| folder);
    editor
        .action("editor.workspace", json!({"folder":folder}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    open_decision(&mut shell, &mut editor);
    let modal_focus = shell.input.focused();
    let revision = editor.host.store().revision();
    shell
        .begin_asset_file_rename(&mut editor, &commands::Target::Asset(asset.path))
        .unwrap();
    assert_eq!(shell.input.focused(), modal_focus);
    shell.position_asset_file_name().unwrap();
    assert_eq!(shell.input.focused(), modal_focus);
    shell.key(&mut editor, 41, true, &mut NoClipboard).unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert_eq!(shell.input.focused(), Some(stable_id("asset-file-name")));
    assert_eq!(editor.host.store().revision(), revision);
}
