//! Docking interaction tests use the real shell, typed layout commands and disk persistence.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_ui::docking::DockNode;
use ara_ui::input::InputEvent;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ara-docking-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Docking","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_docking0","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
    fn open(&self) -> (Editor, Shell) {
        let mut editor = Editor::open(&self.0).unwrap();
        editor
            .action("editor.preferences", json!({"reduceMotion":true}))
            .unwrap();
        let shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
        (editor, shell)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn rect(shell: &Shell, id: &str) -> Box2 {
    shell.ui.nodes[shell.shown_node(id).unwrap()].rect
}
fn centre(r: Box2) -> [f32; 2] {
    [r.x + r.w * 0.5, r.y + r.h * 0.5]
}
fn begin(shell: &mut Shell, panel: &str) {
    let r = shell
        .shown_node(&format!("tab-{panel}"))
        .or_else(|| shell.shown_node(&format!("header-{panel}")))
        .map(|i| shell.ui.nodes[i].rect)
        .unwrap();
    shell.pointer = [r.x + 10., r.y + r.h * 0.5];
    shell.begin_drag();
    assert_eq!(shell.dragged_tab(), Some(panel));
}
fn tabs<'a>(node: &'a DockNode, panel: &str) -> &'a [String] {
    fn find<'a>(node: &'a DockNode, panel: &str) -> Option<&'a [String]> {
        match node {
            DockNode::Area { tabs, .. } => tabs.iter().any(|p| p == panel).then_some(tabs),
            DockNode::Split { first, second, .. } => {
                find(first, panel).or_else(|| find(second, panel))
            }
        }
    }
    find(node, panel).unwrap()
}

#[test]
fn docking_tab_bar_inserts_next_to_game_and_reorders_without_a_split() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    begin(&mut shell, "console");
    let game = rect(&shell, "tab-game");
    shell.pointer = [game.x + game.w - 2., game.y + game.h * 0.5];
    shell.drag_over();
    assert!(matches!(
        shell.drop_preview.as_ref().unwrap().target,
        DropTarget::Tab { index: Some(2), .. }
    ));
    shell.frame().unwrap();
    assert!(
        shell
            .ui
            .display
            .commands
            .iter()
            .any(|d| matches!(d, Draw::Rect { node, rect, .. }
        if *node == stable_id("drop-preview") && rect.w == 2.))
    );
    shell.finish_tab_drag(&mut editor, None).unwrap();
    assert_eq!(
        tabs(&editor.state.layout, "scene"),
        ["scene", "game", "console"]
    );
    begin(&mut shell, "console");
    let scene = rect(&shell, "tab-scene");
    shell.pointer = [scene.x + 2., scene.y + scene.h * 0.5];
    shell.drag_over();
    shell.finish_tab_drag(&mut editor, None).unwrap();
    assert_eq!(
        tabs(&editor.state.layout, "scene"),
        ["console", "scene", "game"]
    );
    // Dropping the first tab after the second counts indices after removing it.
    begin(&mut shell, "console");
    let scene = rect(&shell, "tab-scene");
    shell.pointer = [scene.x + scene.w - 2., scene.y + scene.h * 0.5];
    shell.drag_over();
    shell.finish_tab_drag(&mut editor, None).unwrap();
    assert_eq!(
        tabs(&editor.state.layout, "scene"),
        ["scene", "console", "game"]
    );
    let expected = editor.state.layout.clone();
    drop(shell);
    drop(editor);
    assert_eq!(Editor::open(&project.0).unwrap().state.layout, expected);
}

#[test]
fn docking_edges_preview_the_correct_half_and_centre_adds_a_tab() {
    let project = Project::new();
    let (_, mut shell) = project.open();
    let area = rect(&shell, "area-view");
    for (point, vertical, before) in [
        ([area.x + 2., area.y + area.h * 0.5], false, true),
        ([area.x + area.w - 2., area.y + area.h * 0.5], false, false),
        ([area.x + area.w * 0.5, area.y + 32.], true, true),
        ([area.x + area.w * 0.5, area.y + area.h - 2.], true, false),
    ] {
        shell.pointer = point;
        let (target, preview, _) = shell.drop_target("console").unwrap();
        assert!(
            matches!(target, DropTarget::Split { vertical: v, before: b, .. } if v == vertical && b == before)
        );
        assert_eq!(preview.w, area.w * if vertical { 1. } else { 0.5 });
        assert_eq!(preview.h, area.h * if vertical { 0.5 } else { 1. });
    }
    shell.pointer = centre(area);
    assert!(matches!(
        shell.drop_target("console").unwrap().0,
        DropTarget::Tab { index: None, .. }
    ));
}

#[derive(Default)]
struct Clipboard(String);
impl ara_ui_text::Clipboard for Clipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.into();
        Ok(())
    }
}

#[test]
fn docking_ghost_follows_pointer_with_paint_only_work_and_escape_cancels() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let before = editor.state.dock().unwrap().serialize().unwrap();
    let revision = editor.host.store().revision();
    let mut clipboard = Clipboard::default();
    begin(&mut shell, "console");
    shell
        .input
        .event(
            &mut shell.ui,
            InputEvent::PointerDown(shell.pointer),
            &mut clipboard,
        )
        .unwrap();
    shell.pointer = centre(rect(&shell, "area-view"));
    shell.drag_over();
    shell.frame().unwrap();
    let ghost = shell.ui.find("drop-label").unwrap();
    assert!(shell.ui.nodes[ghost].visible);
    let offset = shell.ui.nodes[ghost].paint_offset;
    shell.pointer[0] += 10.;
    shell.drag_over();
    let work = shell.frame().unwrap();
    assert_ne!(shell.ui.nodes[ghost].paint_offset, offset);
    assert_eq!(work.layout_passes, 0);
    assert_eq!(work.shaped, 0);
    shell.pointer = [-40., -40.];
    shell.drag_over();
    assert!(matches!(
        shell.drop_preview.as_ref().unwrap().target,
        DropTarget::Float
    ));
    shell.key(&mut editor, 41, true, &mut clipboard).unwrap();
    shell.frame().unwrap();
    assert!(!shell.tab_pending());
    assert!(shell.drop_preview.is_none());
    assert!(shell.input.captured().is_none());
    assert_eq!(editor.state.dock().unwrap().serialize().unwrap(), before);
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn docking_on_a_collapsed_bar_opens_the_destination() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor
        .action(
            "editor.layout.collapse",
            json!({"area":"bottom","collapsed":true}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    begin(&mut shell, "game");
    let assets = rect(&shell, "tab-assets");
    shell.pointer = [assets.x + assets.w - 2., assets.y + assets.h * 0.5];
    shell.drag_over();
    shell.finish_tab_drag(&mut editor, None).unwrap();
    assert!(!editor.state.collapsed_areas.contains("bottom"));
    assert_eq!(tabs(&editor.state.layout, "assets")[1], "game");
}

#[test]
fn docking_detached_title_and_tabs_redock_between_windows_then_main() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    for panel in ["assets", "console"] {
        editor
            .action(
                "editor.layout.move",
                json!({"panel":panel,"slot":"float","rect":[150,150,420,560]}),
            )
            .unwrap();
    }
    shell.rebuild(&mut editor).unwrap();
    let source = editor
        .state
        .floating
        .iter()
            .find(|f| matches!(&f.root, DockNode::Area { tabs, .. } if tabs.iter().any(|p| p == "assets")))
        .map(|f| f.id.clone())
        .unwrap();
    let dest = editor
        .state
        .floating
        .iter()
        .find(|f| f.id != source)
        .unwrap()
        .id
        .clone();
    assert!(shell.enter(&source));
    begin(&mut shell, "assets");
    assert!(
        !shell.drag_over(),
        "a floating title press still observes the click threshold"
    );
    assert!(shell.enter(&dest));
    let header = rect(&shell, "header-console");
    shell.pointer = [header.x + 120., header.y + header.h * 0.5];
    shell.drag_over();
    shell.leave();
    shell.finish_tab_drag(&mut editor, None).unwrap();
    assert_eq!(editor.state.floating.len(), 1);
    assert_eq!(
        tabs(&editor.state.floating[0].root, "console"),
        ["console", "assets"]
    );
    assert!(shell.enter(&dest));
    begin(&mut shell, "assets");
    shell.leave();
    let view = rect(&shell, "area-view");
    shell.pointer = [view.x + 2., view.y + view.h * 0.5];
    shell.drag_over();
    shell.finish_tab_drag(&mut editor, None).unwrap();
    assert_eq!(tabs(&editor.state.layout, "assets"), ["assets"]);
    assert!(shell.enter(&dest));
    begin(&mut shell, "console");
    shell.leave();
    shell.pointer = centre(rect(&shell, "area-view"));
    shell.drag_over();
    shell.finish_tab_drag(&mut editor, None).unwrap();
    assert!(editor.state.floating.is_empty());
    let expected = editor.state.layout.clone();
    drop(shell);
    drop(editor);
    assert_eq!(Editor::open(&project.0).unwrap().state.layout, expected);
}
