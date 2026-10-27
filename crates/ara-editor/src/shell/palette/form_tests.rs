#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.

use super::*;
use ara_ui::input::InputEvent;
use ara_ui_text::Clipboard;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let parent = std::path::absolute(crate::test_target())
            .unwrap()
            .join("palette-form-regression");
        std::fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Palette forms","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_palform1","records":{"e_palform1":{"name":"Before undo","Transform":{},"MeshRenderer":{}}}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0.starts_with(
                std::path::absolute(crate::test_target())
                    .unwrap()
                    .join("palette-form-regression")
            )
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}
fn mounted_palette(shell: &mut Shell) {
    shell.frame().unwrap();
    let index = shell
        .shown_node("palette")
        .expect("Palette must be mounted");
    let node = &shell.ui.nodes[index];
    assert_eq!(node.element.tag, "popover");
    assert!(node.rect.intersection(node.clip).w > 0. && node.rect.intersection(node.clip).h > 0.);
}

#[test]
fn monkey_component_fallback_then_held_pointer_undo_keeps_palette_markup_well_formed() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_palform1","path":"name","value":"Undo this change"}})], "Palette regression undo target", None).unwrap();
    editor
        .action("editor.select", json!({"entities":[]}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell.frame().unwrap();
    assert!(shell.shown_node("add-component").is_none());
    let revision = editor.host.store().revision();
    // Seed104729 steps4694/4695 first open the fallback with edge-case targets.
    // A reference may be refused; it must never leave globally invalid markup.
    for entity in ["", "@levels/deleted.scene#e_deleted1"] {
        if let Err(error) =
            shell.run_command(&mut editor, "component.add", &Target::Entity(entity.into()))
        {
            assert!(
                !error.to_string().contains("ARA4001"),
                "Component fallback generated invalid markup: {error}"
            );
        }
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
    }
    // Also exercise the valid fallback, independent of future reference validation.
    editor
        .action("editor.select", json!({"entities":["e_palform1"]}))
        .unwrap();
    editor
        .action("editor.layout.close", json!({"panel":"inspector"}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell
        .run_command(&mut editor, "component.add", &Target::None)
        .unwrap();
    assert_eq!(shell.palette_command.as_deref(), Some("@components"));
    mounted_palette(&mut shell);
    assert!(shell.shown_node("palette-group-components").is_some());
    assert_eq!(
        editor.host.store().revision(),
        revision,
        "Opening a generated form is not an authoring mutation"
    );
    let mut clip = Clip::default();
    // The fatal rebuild at4723 followed this actual pointer chord at4721/4722.
    shell
        .event(InputEvent::PointerDown([720., 450.]), &mut clip)
        .unwrap();
    shell
        .event(InputEvent::PointerMove([752., 474.]), &mut clip)
        .unwrap();
    shell
        .run_command(&mut editor, "edit.undo", &Target::None)
        .unwrap();
    mounted_palette(&mut shell);
    assert_eq!(
        editor
            .host
            .store()
            .workspace()
            .doc("sc_palform1")
            .unwrap()
            .record("e_palform1")
            .unwrap()["name"],
        "Before undo"
    );
    assert_eq!(editor.host.store().revision(), revision + 1);
    shell
        .event(InputEvent::PointerUp([752., 474.]), &mut clip)
        .unwrap();
    shell.key(&mut editor, 41, true, &mut clip).unwrap();
    shell.frame().unwrap();
    assert!(!editor.state.palette);
    assert!(shell.input.captured().is_none());
}

#[test]
fn native_palette_result_opens_a_generated_engine_form_and_escape_leaves_authoring_unchanged() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_palform1"]}))
        .unwrap();
    editor
        .action("editor.palette", json!({"open":true}))
        .unwrap();
    editor
        .action(
            "editor.search",
            json!({"panel":"palette","query":">transform.set"}),
        )
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    mounted_palette(&mut shell);
    let id = shell
        .ui
        .nodes
        .iter()
        .find(|node| {
            node.element.tag == "button"
                && node
                    .element
                    .attrs
                    .get("label")
                    .is_some_and(|label| label == "transform.set")
        })
        .expect("Registered engine command result")
        .element
        .id
        .clone();
    let revision = editor.host.store().revision();
    let mut clip = Clip::default();
    shell.click(&id, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    mounted_palette(&mut shell);
    assert_eq!(shell.palette_command.as_deref(), Some("transform.set"));
    assert!(shell.shown_node("palette-run").is_some());
    assert!(shell.shown_node("param-entities").is_some());
    shell.key(&mut editor, 41, true, &mut clip).unwrap();
    shell.frame().unwrap();
    assert!(!editor.state.palette);
    assert_eq!(editor.host.store().revision(), revision);
}
