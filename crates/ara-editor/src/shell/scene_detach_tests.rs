//! One Scene follows its owning retained surface through detach, resize and redock.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_ui::{docking::DockNode, paint::Draw};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"))
            .join("scene-detach")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Scene detach","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_detach01","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }

    fn open(&self) -> (Editor, Shell) {
        let mut editor =
            Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap();
        editor
            .action("editor.preferences", json!({"reduceMotion":true}))
            .unwrap();
        let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
        shell.offscreen = true;
        (editor, shell)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

fn begin(shell: &mut Shell, panel: &str) {
    let index = shell
        .shown_node(&format!("tab-{panel}"))
        .or_else(|| shell.shown_node(&format!("header-{panel}")))
        .expect("shown tab or panel title");
    let rect = shell.ui.nodes[index].rect;
    shell.pointer = [rect.x + 10., rect.y + rect.h * 0.5];
    shell.begin_drag();
    assert_eq!(shell.dragged_tab(), Some(panel));
}

fn centre(rect: Box2) -> [f32; 2] {
    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]
}

fn scene_image(shell: &Shell) -> Box2 {
    let viewport = shell.viewport().expect("current surface hosts Scene");
    assert!(viewport.w > 200. && viewport.h > 100.);
    assert!(shell.ui.display.commands.iter().any(|draw| {
        matches!(draw, Draw::Image { node, image, rect, .. }
            if *node == stable_id("viewport-image")
                && *image == overlay::viewport_image()
                && rect.w == viewport.w && rect.h == viewport.h)
    }));
    viewport
}

#[test]
fn scene_tab_drag_cancels_then_detaches_resizes_and_redocks_without_authoring_ops() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let original = editor.state.dock().unwrap().serialize().unwrap();
    let revision = editor.host.store().revision();
    let camera = editor.state.camera;
    let main = scene_image(&shell);
    let expected_ray = overlay::ground_point(&camera, main, centre(main)).unwrap();

    begin(&mut shell, "scene");
    assert!(
        !shell.drag_over(),
        "title press observes the drag threshold"
    );
    shell.pointer = [-40., -40.];
    assert!(shell.drag_over());
    assert!(matches!(
        shell.drop_preview.as_ref().unwrap().target,
        DropTarget::Float
    ));
    shell
        .key(&mut editor, 41, true, &mut Clipboard::default())
        .unwrap();
    assert!(!shell.tab_pending());
    assert_eq!(editor.state.dock().unwrap().serialize().unwrap(), original);

    begin(&mut shell, "scene");
    shell.pointer = [-40., -40.];
    shell.drag_over();
    shell
        .finish_tab_drag(&mut editor, Some([160, 120, 960, 640]))
        .unwrap();
    assert!(
        shell.viewport().is_none(),
        "main keeps no stale picking rectangle"
    );
    let id = editor.state.floating[0].id.clone();
    assert_eq!(shell.scene_host().unwrap().0, Some(id.as_str()));
    assert!(shell.enter(&id));
    let first = scene_image(&shell);
    assert_eq!(shell.scene_host().unwrap().0, Some(id.as_str()));
    assert!(
        (overlay::ground_point(&camera, first, centre(first)).unwrap() - expected_ray).length()
            < 0.0001,
        "floating pointer rays use the host-local image origin and extent"
    );

    shell
        .click("tool-Rotate", &mut Clipboard::default())
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(editor.state.tool, crate::state::Tool::Rotate);
    shell.resize_surface(&id, [1180., 760.]).unwrap();
    shell.frame().unwrap();
    shell.orient_axis(editor.state.camera);
    shell.frame().unwrap();
    let resized = scene_image(&shell);
    assert!(resized.w > first.w && resized.h > first.h);
    let cube = shell.ui.nodes[shell.shown_node("view-cube").unwrap()].rect;
    assert!((cube.x + cube.w - (resized.x + resized.w - 12.)).abs() <= 0.5);
    assert!((cube.y - (resized.y + 12.)).abs() <= 0.5);

    begin(&mut shell, "scene");
    shell.leave();
    let game = shell.ui.nodes[shell
        .shown_node("tab-game")
        .or_else(|| shell.shown_node("header-game"))
        .expect("remaining Game panel has a visible tab or lone-panel header")]
    .rect;
    shell.pointer = [game.x + 2., game.y + game.h * 0.5];
    assert!(shell.drag_over());
    assert!(matches!(
        shell.drop_preview.as_ref().unwrap().target,
        DropTarget::Tab { .. }
    ));
    shell.finish_tab_drag(&mut editor, None).unwrap();
    assert!(editor.state.floating.is_empty());
    assert_eq!(shell.scene_host().unwrap().0, None);
    scene_image(&shell);
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(editor.state.camera, camera);
}

#[test]
fn scene_window_command_restores_saved_host_and_redocking_preserves_peer_panels() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    shell
        .execute(
            &mut editor,
            "window.float",
            &commands::Target::Panel("scene".into()),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let id = editor.state.floating[0].id.clone();
    let area = match &editor.state.floating[0].root {
        DockNode::Area { id, .. } => id.clone(),
        DockNode::Split { .. } => panic!("single floated Scene begins in an area"),
    };
    editor
        .action("editor.layout", json!({"panel":"console","area":area}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(
        shell.scene_host().is_none(),
        "an inactive retained Scene does not supply stale render or picking geometry"
    );
    editor
        .action("editor.layout.show", json!({"panel":"scene"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let saved = editor.state.dock().unwrap().serialize().unwrap();
    drop(shell);
    drop(editor);

    let (mut editor, mut shell) = project.open();
    assert_eq!(editor.state.dock().unwrap().serialize().unwrap(), saved);
    assert_eq!(shell.scene_host().unwrap().0, Some(id.as_str()));
    assert!(shell.enter(&id));
    scene_image(&shell);
    shell.leave();
    // The native close path uses this same typed move before closing other tabs.
    editor
        .action("editor.layout.move", json!({"panel":"scene","slot":"view"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(shell.scene_host().unwrap().0, None);
    scene_image(&shell);
    assert_eq!(editor.state.floating.len(), 1);
    assert!(shell.enter(&id));
    assert!(shell.viewport().is_none());
    assert_eq!(shell.scene_host().unwrap().0, None);
    assert!(shell.shown_node("console-toolbar").is_some());
    assert!(editor.state.dock().unwrap().contains("console"));
}
