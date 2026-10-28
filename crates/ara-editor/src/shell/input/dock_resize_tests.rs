//! Real splitter events persist only their owning surface through typed navigation ops.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_ui::docking::{DockLayout, DockNode};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("floating-split-resize")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.parent().unwrap()).unwrap();
        std::fs::create_dir(&root).unwrap();
        let content = root.join(ara_doc::AUTHORING_ROOT);
        std::fs::create_dir_all(content.join("levels")).unwrap();
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"name":"Floating split test","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"main":"levels/main.scene"}),
            ),
        )
        .unwrap();
        std::fs::write(
            content.join("levels/main.scene"),
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_floatresize","records":{}}),
            ),
        )
        .unwrap();
        Self(root)
    }
    fn editor(&self) -> Editor {
        Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0.starts_with(
                std::path::absolute(crate::test_target())
                    .unwrap()
                    .join("floating-split-resize")
            )
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Clip;
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _value: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
fn float_split(editor: &mut Editor) -> String {
    editor
        .action(
            "editor.layout.move",
            json!({"panel":"inspector","slot":"float","rect":[200,180,760,600]}),
        )
        .unwrap();
    let window = editor.state.floating.last().unwrap().id.clone();
    let area = match &editor.state.floating.last().unwrap().root {
        DockNode::Area { id, .. } => id.clone(),
        _ => panic!("Freshly floated panel is an area"),
    };
    editor.action("editor.layout",json!({"panel":"console","area":area,"newArea":"floating-console","vertical":true,"before":false})).unwrap();
    window
}
fn root_ratio(editor: &Editor, window: &str) -> f32 {
    let root = &editor
        .state
        .floating
        .iter()
        .find(|floating| floating.id == window)
        .unwrap()
        .root;
    match root {
        DockNode::Split { ratio, .. } => *ratio,
        _ => panic!("Expected a floating split"),
    }
}
fn history(editor: &mut Editor) -> usize {
    editor.action("editor.history", json!({})).unwrap()["items"]
        .as_array()
        .unwrap()
        .len()
}

#[test]
fn floating_splitter_pointer_drag_uses_typed_navigation_and_survives_editor_reopen() {
    let project = Project::new();
    let mut editor = project.editor();
    let window = float_split(&mut editor);
    let main = editor.state.layout.clone();
    let content_revision = editor.host.store().revision();
    let before_history = history(&mut editor);
    let before_ratio = root_ratio(&editor, &window);
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell.offscreen = true;
    assert!(shell.enter(&window));
    shell.frame().unwrap();
    let id = format!("splitter-f{window}");
    let index = shell.shown_node(&id).unwrap();
    let rect = shell.ui.nodes[index]
        .rect
        .intersection(shell.ui.nodes[index].clip);
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    assert!(rect.w > 1. && rect.h > 1.);
    assert_eq!(shell.ui.hit_test(point[0], point[1]), Some(index));
    let parent = shell.ui.nodes[index].element.parent.unwrap();
    let extent = shell.ui.nodes[parent].rect;
    let destination = [point[0], extent.y + extent.h * 0.7];
    let mut clip = Clip;
    shell
        .event(InputEvent::PointerDown(point), &mut clip)
        .unwrap();
    assert_eq!(shell.input.captured(), Some(ara_ui::source::stable_id(&id)));
    shell
        .event(InputEvent::PointerMove(destination), &mut clip)
        .unwrap();
    shell
        .event(InputEvent::PointerUp(destination), &mut clip)
        .unwrap();
    assert!(shell.dispatch(&mut editor).unwrap());
    shell.frame().unwrap();
    let ratio = root_ratio(&editor, &window);
    assert!((ratio - 0.7).abs() < 0.002);
    assert!((ratio - before_ratio).abs() > 0.1);
    assert_eq!(editor.state.layout, main);
    assert_eq!(editor.host.store().revision(), content_revision);
    assert!(history(&mut editor) > before_history);
    assert_eq!(shell.current(), Some(window.as_str()));
    let expected = editor.state.dock().unwrap().serialize().unwrap();
    drop(shell);
    drop(editor);
    let restored = project.editor();
    assert_eq!(restored.state.layout, main);
    assert_eq!(root_ratio(&restored, &window), ratio);
    assert_eq!(
        restored.state.dock().unwrap().serialize().unwrap(),
        expected
    );
}

#[test]
fn invalid_floating_resize_requests_leave_layout_navigation_history_and_authoring_unchanged() {
    let project = Project::new();
    let mut editor = project.editor();
    let window = float_split(&mut editor);
    let before = editor.state.dock().unwrap().serialize().unwrap();
    let before_history = history(&mut editor);
    let content_revision = editor.host.store().revision();
    for params in [
        json!({"window":"missing","ratio":0.6}),
        json!({"window":"","ratio":0.6}),
        json!({"window":window,"path":[true],"ratio":0.6}),
        json!({"window":window,"path":[true,false],"ratio":0.6}),
        json!({"window":window}),
        json!({"window":window,"ratio":"NaN"}),
        json!({"window":window,"ratio":1e300}),
    ] {
        assert!(editor.action("editor.layout", params).is_err());
        assert_eq!(editor.state.dock().unwrap().serialize().unwrap(), before);
        assert_eq!(history(&mut editor), before_history);
        assert_eq!(editor.host.store().revision(), content_revision);
    }
}

#[test]
fn omitted_or_null_window_preserves_legacy_main_split_resize_behavior() {
    let project = Project::new();
    let mut editor = project.editor();
    let window = float_split(&mut editor);
    let floating = editor.state.floating.clone();
    for params in [
        json!({"path":[],"ratio":0.37}),
        json!({"window":null,"path":[],"ratio":0.37}),
    ] {
        editor.action("editor.layout", params).unwrap();
        assert!(
            matches!(&editor.state.layout,DockNode::Split {ratio,..} if (*ratio-0.37).abs()<0.0001)
        );
        assert_eq!(editor.state.floating, floating);
        assert_eq!(root_ratio(&editor, &window), 0.5);
    }
    let mut restored = DockLayout::new(editor.state.layout.clone()).unwrap();
    restored
        .restore(&editor.state.dock().unwrap().serialize().unwrap())
        .unwrap();
    assert_eq!(restored.floating(), floating.as_slice());
}
