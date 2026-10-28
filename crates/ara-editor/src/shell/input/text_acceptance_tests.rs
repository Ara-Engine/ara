//! Real Inspector commit and focused keyboard history, with no synthetic authoring shortcut.
use super::*;
use ara_ui::{input::InputEvent, source::stable_id, widgets::Widget};
use ara_ui_text::{Clipboard, EditEvent};
use std::sync::atomic::{AtomicU64, Ordering};
struct Clip;
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
fn open() -> (Editor, Shell) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = crate::test_target().join("text-acceptance").join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels"))
        .expect("scratch directory");
    std::fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&json!({"name":"Accepted text","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"main":"levels/main.scene"}))).expect("marker");
    std::fs::write(root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),ara_json::to_canonical_string(&json!({"$schema":"ara://schema/scene/1","id":"sc_textaccept","records":{
 "e_textaccept1":{"name":"First","Transform":{},"MeshRenderer":{"emissiveMaterials":["first0","first1"]},"Light":{"intensity":1000}},
 "e_textaccept2":{"name":"Second","Transform":{},"MeshRenderer":{"emissiveMaterials":["second0"]},"Light":{"intensity":2000}}}}))).expect("scene");
    let mut editor =
        Editor::open_with(&root, Some(root.join(".ara/accept-user.json"))).expect("authority");
    editor
        .action(
            "editor.select",
            json!({"entities":["e_textaccept1","e_textaccept2"]}),
        )
        .expect("selection");
    editor
        .action("editor.view", json!({"maximized":"inspector"}))
        .expect("maximize");
    editor
        .action(
            "editor.search",
            json!({"panel":"inspector","query":"MeshRenderer.emissiveMaterials"}),
        )
        .expect("filter");
    let mut shell = Shell::new(&mut editor, [1000., 700.]).expect("shell");
    shell.offscreen = true;
    shell.frame().expect("layout");
    (editor, shell)
}
fn records(editor: &Editor) -> Value {
    let doc = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .expect("scene");
    json!([doc.record("e_textaccept1"), doc.record("e_textaccept2")])
}
fn replace(editor: &mut Editor, shell: &mut Shell, id: &str, value: &str) {
    shell.click(id, &mut Clip).expect("field click");
    shell
        .event(InputEvent::Edit(EditEvent::SelectAll), &mut Clip)
        .expect("select all");
    shell
        .event(InputEvent::Edit(EditEvent::Insert(value.into())), &mut Clip)
        .expect("typing");
    shell.dispatch(editor).expect("draft dispatch");
}
fn chord(editor: &mut Editor, shell: &mut Shell, key: i32) {
    let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
    shell
        .key(editor, primary, true, &mut Clip)
        .expect("modifier");
    shell.key(editor, key, true, &mut Clip).expect("key");
    shell.key(editor, key, false, &mut Clip).expect("key up");
    shell
        .key(editor, primary, false, &mut Clip)
        .expect("modifier up");
    shell.dispatch(editor).expect("dispatch");
}
const FIELD: &str = "nest-edit-MeshRenderer-emissiveMaterials-0";
#[test]
fn accepted_enter_keeps_focus_but_one_ctrl_z_and_ctrl_y_use_typed_authoring_history() {
    let (mut editor, mut shell) = open();
    let before = records(&editor);
    let revision = editor.host.store().revision();
    replace(&mut editor, &mut shell, FIELD, "accepted");
    shell
        .key(&mut editor, 40, true, &mut Clip)
        .expect("accepted Enter");
    assert_eq!(editor.host.store().revision(), revision + 1);
    let after = records(&editor);
    assert_ne!(after, before);
    assert_eq!(shell.input.focused(), Some(stable_id(FIELD)));
    assert!(
        matches!(shell.ui.widgets.get(&stable_id(FIELD)),Some(Widget::Input(input)) if !input.can_undo() && !input.can_redo())
    );
    chord(&mut editor, &mut shell, 29);
    assert_eq!(editor.host.store().revision(), revision + 2);
    assert_eq!(records(&editor), before);
    assert_eq!(shell.input.focused(), Some(stable_id(FIELD)));
    chord(&mut editor, &mut shell, 28);
    assert_eq!(editor.host.store().revision(), revision + 3);
    assert_eq!(records(&editor), after);
}
#[test]
fn uncommitted_draft_undo_and_redo_remain_local_without_model_history() {
    let (mut editor, mut shell) = open();
    let before = records(&editor);
    let revision = editor.host.store().revision();
    replace(&mut editor, &mut shell, FIELD, "draft only");
    chord(&mut editor, &mut shell, 29);
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(records(&editor), before);
    assert!(
        matches!(shell.ui.widgets.get(&stable_id(FIELD)),Some(Widget::Input(input)) if input.value()!="draft only" && input.can_redo())
    );
    chord(&mut editor, &mut shell, 28);
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(records(&editor), before);
    assert!(
        matches!(shell.ui.widgets.get(&stable_id(FIELD)),Some(Widget::Input(input)) if input.value()=="draft only")
    );
}
#[test]
fn rejected_validation_does_not_acknowledge_or_discard_draft_history() {
    let (mut editor, mut shell) = open();
    editor
        .action(
            "editor.search",
            json!({"panel":"inspector","query":"Light.intensity"}),
        )
        .expect("numeric filter");
    shell.rebuild(&mut editor).expect("rebuild");
    shell.frame().expect("frame");
    let before = records(&editor);
    let revision = editor.host.store().revision();
    let id = "edit-Light-intensity";
    replace(&mut editor, &mut shell, id, "not-a-number");
    assert!(shell.key(&mut editor, 40, true, &mut Clip).is_err());
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(records(&editor), before);
    assert!(
        matches!(shell.ui.widgets.get(&stable_id(id)),Some(Widget::Input(input)) if input.can_undo())
    );
    chord(&mut editor, &mut shell, 29);
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(records(&editor), before);
    assert!(
        matches!(shell.ui.widgets.get(&stable_id(id)),Some(Widget::Input(input)) if input.value()!="not-a-number")
    );
}
