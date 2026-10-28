//! Real authority and retained event routing, with inventory held by a test channel.
use super::*;
use ara_ui::{input::InputEvent, source::stable_id, widgets::Widget};
use std::sync::atomic::{AtomicU64, Ordering};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target()).unwrap()
            .join("script-module-discovery")
            .join(format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        for (path, value) in [
            ("ara.project.json", json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Module chooser","main":"levels/main.scene"})),
            ("levels/main.scene", json!({"$schema":"ara://schema/scene/1","id":"sc_modules","records":{}})),
        ] {
            let path = if path == "ara.project.json" { root.join(path) }
                else { root.join(ara_doc::AUTHORING_ROOT).join(path) };
            std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn complete(shell: &mut Shell, editor: &mut Editor) {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while shell.script_module_busy() {
        editor.tick();
        if shell.poll_script_modules(editor).unwrap() {
            shell.rebuild(editor).unwrap();
        }
        assert!(Instant::now() < deadline, "controlled worker failed to retire");
        std::thread::yield_now();
    }
    shell.frame().unwrap();
}

fn visible(shell: &Shell, id: &str) -> ara_ui::paint::Box2 {
    let index = shell.shown_node(id).unwrap_or_else(|| panic!("missing {id}"));
    let node = &shell.ui.nodes[index];
    let rect = node.rect.intersection(node.clip);
    assert!(rect.w > 0. && rect.h > 0., "{id}: {:?} clip {:?}", node.rect, node.clip);
    rect
}

fn click(shell: &mut Shell, editor: &mut Editor, id: &str) {
    let rect = visible(shell, id);
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    assert_eq!(shell.ui.hit_test(point[0], point[1]).map(|index| shell.ui.nodes[index].element.id.as_str()), Some(id));
    shell.event(InputEvent::PointerDown(point), &mut NoClipboard).unwrap();
    shell.event(InputEvent::PointerUp(point), &mut NoClipboard).unwrap();
    shell.dispatch(editor).unwrap();
    shell.frame().unwrap();
}

#[test]
fn blocked_inventory_keeps_editor_ticks_and_retained_unicode_typing_live_and_cancel_prevents_late_open() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
    shell.offscreen = true;
    shell.frame().unwrap();
    let original = stable_id("search-hierarchy");
    shell.input.focus(&mut shell.ui, original).unwrap();
    let revision = editor.host.store().revision();
    let release = blocked(&mut shell.script_modules, editor.host.root());
    for _ in 0..16 {
        shell.open_script_solution(&mut editor).unwrap();
        editor.tick();
        assert!(!shell.poll_script_modules(&mut editor).unwrap());
        shell.frame().unwrap();
    }
    assert_eq!(shell.script_module_job_report()["started"], 1);
    visible(&shell, "dialog-cancel");
    assert!(shell.script_module_busy());
    assert_eq!(shell.script_module_job_report()["pendingPresentation"], true);
    assert_eq!(editor.host.store().revision(), revision);
    shell.key(&mut editor, 41, true, &mut NoClipboard).unwrap();
    shell.frame().unwrap();
    assert!(shell.dialog.is_none());
    assert_eq!(shell.input.focused(), Some(original));
    assert_eq!(shell.script_module_job_report()["workerRetired"], false);
    assert_eq!(shell.script_module_job_report()["pendingPresentation"], false);
    shell.native_text("λص", &mut NoClipboard).unwrap();
    shell.dispatch(&mut editor).unwrap();
    editor.tick();
    shell.rebuild_if_stale(&mut editor).unwrap();
    shell.frame().unwrap();
    let Widget::Input(input) = &shell.ui.widgets[&original] else { panic!("live search field") };
    assert_eq!(input.value(), "λص");
    let status = shell.status_note.clone();
    release.send(Ok(vec![editor.host.root().join("modules/game")])).unwrap();
    complete(&mut shell, &mut editor);
    assert!(shell.dialog.is_none());
    assert_eq!(shell.input.focused(), Some(original));
    assert_eq!(shell.status_note, status, "a cancelled discovery never opens an IDE after completion");
    assert_eq!(shell.script_module_job_report()["completed"], 1);
    assert_eq!(shell.script_module_job_report()["workerRetired"], true);
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn module_paths_use_real_searchable_select_in_narrow_modal_and_escape_restores_focus() {
    for light in [false, true] {
        let project = Project::new();
        let mut editor = Editor::open(&project.0).unwrap();
        editor.action("editor.theme", json!({"light":light})).unwrap();
        let mut shell = Shell::new(&mut editor, [320., 450.]).unwrap();
        shell.offscreen = true;
        shell.frame().unwrap();
        let original = shell.input.focused();
        let release = blocked(&mut shell.script_modules, editor.host.root());
        shell.open_script_solution(&mut editor).unwrap();
        let modules: Vec<_> = (0..12).map(|index| editor.host.root().join(format!("modules/{index:02}-long-localized-configuration-إعدادات/game.module"))).collect();
        release.send(Ok(modules)).unwrap();
        complete(&mut shell, &mut editor);
        let rect = visible(&shell, "dialog-body");
        assert!(rect.x >= 0. && rect.y >= 0. && rect.x + rect.w <= 320.5 && rect.y + rect.h <= 450.5);
        let body = visible(&shell, "script-module-content");
        assert!(body.h > 1.);
        click(&mut shell, &mut editor, "script-module-choice");
        let search = "choice-script-module-choice-search";
        visible(&shell, search);
        assert_eq!(shell.input.focused(), Some(stable_id(search)));
        shell.native_text("11-long", &mut NoClipboard).unwrap();
        shell.dispatch(&mut editor).unwrap();
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        shell.key(&mut editor, 77, true, &mut NoClipboard).unwrap();
        shell.frame().unwrap();
        let index = shell.ui.nodes.iter().position(|node| {
            node.element.id.starts_with("choice-script-module-choice-option-")
                && node.element.id.ends_with("-11")
        }).unwrap();
        let node = &shell.ui.nodes[index];
        assert!((node.rect.intersection(node.clip).h - node.rect.h).abs() < 0.5, "last matching module must be fully revealed");
        shell.key(&mut editor, 40, true, &mut NoClipboard).unwrap();
        shell.frame().unwrap();
        let Some(Dialog::ScriptModules { selected, .. }) = &shell.dialog else { panic!("choosing does not open the IDE") };
        assert!(selected.as_deref().unwrap().contains("11-long"));
        click(&mut shell, &mut editor, "script-module-choice");
        shell.key(&mut editor, 41, true, &mut NoClipboard).unwrap();
        assert!(shell.dialog.is_some(), "first Escape closes only the Select");
        shell.key(&mut editor, 41, true, &mut NoClipboard).unwrap();
        shell.frame().unwrap();
        assert!(shell.dialog.is_none());
        assert_eq!(shell.input.focused(), original);
        assert!(!shell.script_module_busy());
    }
}

#[test]
fn discovery_error_is_visible_with_last_good_paths_and_only_explicit_open_confirms() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [900., 650.]).unwrap();
    shell.offscreen = true;
    let release = blocked(&mut shell.script_modules, editor.host.root());
    shell.open_script_solution(&mut editor).unwrap();
    release.send(Ok(vec![editor.host.root().join("modules/game")])).unwrap();
    complete(&mut shell, &mut editor);
    let release = blocked(&mut shell.script_modules, editor.host.root());
    click(&mut shell, &mut editor, "dialog-script-refresh");
    release.send(Err(vec![ara_diag::Diagnostic::error(ara_script::codes::UNKNOWN_MODULE, "Missing dependency in module inventory")])).unwrap();
    complete(&mut shell, &mut editor);
    visible(&shell, "script-module-error");
    visible(&shell, "script-module-choice");
    assert_eq!(shell.script_module_job_report()["lastGoodModules"], 1);
    assert!(shell.dialog.is_some());
    click(&mut shell, &mut editor, "dialog-script-open");
    assert!(shell.dialog.is_none());
    assert!(shell.status_note.as_deref().unwrap().contains("modules/game"));
    assert!(shell.status_note.as_deref().unwrap().contains("disabled"));
}
