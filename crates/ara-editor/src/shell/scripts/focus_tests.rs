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
                "$schema":"ara://schema/scene/1","id":ara_doc::AssetId::parse("sc_renamefocus").unwrap(),"records":{}
            })),
        )
        .unwrap();
        Self(root)
    }
    fn open(&self) -> Editor {
        Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap()
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
fn await_scene_index(shell: &mut Shell, editor: &mut Editor) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        shell.poll_assets();
        if shell
            .browser
            .items
            .iter()
            .any(|asset| asset.kind == "scene")
        {
            shell.rebuild(editor).unwrap();
            shell.frame().unwrap();
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real authored scene index did not publish: {:?}",
            shell.browser.error
        );
        editor.tick();
        std::thread::yield_now();
    }
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
    let mut editor = project.open();
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
    let path = format!("{}/NewSystem.cs", ara_doc::AUTHORING_ROOT);
    shell.browser.insert_path(&path, "cs");
    editor
        .action(
            "editor.workspace",
            json!({"folder":ara_doc::AUTHORING_ROOT,"asset":path}),
        )
        .unwrap();
    shell.asset_rename = Some(Rename {
        path,
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
    // A short bottom drawer may clip the card's name. Reveal that actual input
    // while the decision still owns focus; deferred restoration must not focus
    // an offscreen field merely because its retained node exists.
    editor
        .action("editor.view", json!({"maximized":"assets"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert_eq!(shell.input.focused(), Some(search));
    let field = &shell.ui.nodes[shell.shown_node("asset-rename-value").unwrap()];
    assert!(
        field.rect.intersection(field.clip).w > 0.
            && field.rect.intersection(field.clip).h >= field.rect.h - 0.5,
        "rename fixture must be visible before restoration: {:?}, clip {:?}",
        field.rect,
        field.clip
    );
    assert!(
        shell.input.focus(&mut shell.ui, rename).is_err(),
        "revealing the background rename must not bypass the active Select/modal trap"
    );
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
    let mut editor = project.open();
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"assets","slot":"view"}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [900., 650.]).unwrap();
    shell.offscreen = true;
    await_scene_index(&mut shell, &mut editor);
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
