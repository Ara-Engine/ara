#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.

use super::*;
use ara_ui::input::InputEvent;
use ara_ui::{source::stable_id, widgets::Widget};
use ara_ui_text::Clipboard;
use ara_ui_text::EditEvent;
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

fn result_id(shell: &Shell, command: &str) -> String {
    shell
        .ui
        .nodes
        .iter()
        .find(|node| {
            node.element.tag == "button"
                && node
                    .element
                    .attrs
                    .get("label")
                    .is_some_and(|label| label == command)
        })
        .expect("Registered engine command result")
        .element
        .id
        .clone()
}
fn open_form(shell: &mut Shell, editor: &mut Editor, command: &str, clip: &mut Clip) {
    shell
        .run_command(editor, "edit.palette", &Target::None)
        .unwrap();
    editor
        .action(
            "editor.search",
            json!({"panel":"palette","query":format!(">{command}")}),
        )
        .unwrap();
    shell.rebuild(editor).unwrap();
    shell.frame().unwrap();
    let id = result_id(shell, command);
    shell.click(&id, clip).unwrap();
    shell.dispatch(editor).unwrap();
    shell.frame().unwrap();
    assert_eq!(shell.palette_command.as_deref(), Some(command));
}
fn point(shell: &mut Shell, editor: &mut Editor, id: &str, clip: &mut Clip) -> [f32; 2] {
    for _ in 0..48 {
        let index = shell.shown_node(id).expect("Parameter control exists");
        let node = &shell.ui.nodes[index];
        let visible = node.rect.intersection(node.clip);
        if visible.w > 1. && visible.h >= (node.rect.h - 0.5).min(24.) {
            assert!(
                node.rect.x >= -0.5 && node.rect.x + node.rect.w <= shell.ui.size[0] + 0.5,
                "{id}: {:?} outside {:?}",
                node.rect,
                shell.ui.size
            );
            return [visible.x + visible.w * 0.5, visible.y + visible.h * 0.5];
        }
        let delta = if node.rect.y < node.clip.y { -64. } else { 64. };
        let scroll = shell
            .ui
            .find("palette-form-scroll")
            .expect("Scrolling form body");
        let body = shell.ui.nodes[scroll]
            .rect
            .intersection(shell.ui.nodes[scroll].clip);
        assert!(
            body.h > 1. && body.w > 1.,
            "Form body must not collapse: {body:?}"
        );
        shell
            .event(
                InputEvent::Wheel {
                    position: [body.x + body.w * 0.5, body.y + body.h * 0.5],
                    delta: [0., delta],
                },
                clip,
            )
            .unwrap();
        shell.dispatch(editor).unwrap();
        shell.frame().unwrap();
    }
    panic!("{id} was not reachable by native wheel input");
}
fn click_control(shell: &mut Shell, editor: &mut Editor, id: &str, clip: &mut Clip) {
    let point = point(shell, editor, id, clip);
    shell.event(InputEvent::PointerDown(point), clip).unwrap();
    shell.event(InputEvent::PointerUp(point), clip).unwrap();
    shell.dispatch(editor).unwrap();
    shell.frame().unwrap();
}
fn type_control(shell: &mut Shell, editor: &mut Editor, id: &str, text: &str, clip: &mut Clip) {
    click_control(shell, editor, id, clip);
    let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
    shell.key(editor, primary, true, clip).unwrap();
    shell.key(editor, 4, true, clip).unwrap();
    shell.key(editor, primary, false, clip).unwrap();
    shell
        .event(InputEvent::Edit(EditEvent::Insert(text.into())), clip)
        .unwrap();
    shell.dispatch(editor).unwrap();
    shell.frame().unwrap();
}

#[test]
fn generated_import_form_uses_select_toggle_plain_strings_and_native_back_focus_in_both_themes() {
    for light in [false, true] {
        let project = Project::new();
        let mut editor = Editor::open(&project.0).unwrap();
        editor
            .action("editor.theme", json!({"light":light}))
            .unwrap();
        let mut shell = Shell::new(&mut editor, [160., 450.]).unwrap();
        let mut clip = Clip::default();
        let revision = editor.host.store().revision();
        open_form(&mut shell, &mut editor, "asset.import", &mut clip);
        assert_eq!(shell.input.focused(), Some(stable_id("param-sources")));
        let index = shell.ui.find("param-conflict").unwrap();
        assert_eq!(shell.ui.nodes[index].element.tag, "dropdown");
        let index = shell.ui.find("param-dryRun").unwrap();
        assert_eq!(shell.ui.nodes[index].element.tag, "toggle");
        click_control(&mut shell, &mut editor, "param-conflict", &mut clip);
        assert!(shell.choice_open());
        shell.key(&mut editor, 77, true, &mut clip).unwrap();
        shell.key(&mut editor, 40, true, &mut clip).unwrap();
        assert_eq!(shell.palette_params["conflict"], "skip");
        assert!(!shell.choice_open());
        click_control(&mut shell, &mut editor, "param-dryRun", &mut clip);
        assert_eq!(shell.palette_params["dryRun"], true);
        type_control(
            &mut shell,
            &mut editor,
            "param-destination",
            "false",
            &mut clip,
        );
        assert_eq!(
            shell.palette_params["destination"],
            Value::String("false".into())
        );
        assert_eq!(shell.input.focused(), Some(stable_id("param-destination")));
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert_eq!(shell.input.focused(), Some(stable_id("param-destination")));
        let Widget::Input(input) = &shell.ui.widgets[&stable_id("param-destination")] else {
            panic!("Destination field missing")
        };
        assert_eq!(
            input.value(),
            "false",
            "String fields must not acquire JSON quotes or Boolean coercion"
        );
        click_control(&mut shell, &mut editor, "palette-back", &mut clip);
        assert!(shell.palette_command.is_none());
        assert_eq!(shell.input.focused(), Some(stable_id("palette-search")));
        let id = result_id(&shell, "asset.import");
        shell.click(&id, &mut clip).unwrap();
        shell.dispatch(&mut editor).unwrap();
        shell.frame().unwrap();
        assert_eq!(
            shell.palette_params["destination"], "false",
            "Back/re-entry should retain the same form's draft"
        );
        assert_eq!(shell.palette_params["dryRun"], true);
        type_control(
            &mut shell,
            &mut editor,
            "param-sources",
            "not JSON",
            &mut clip,
        );
        click_control(&mut shell, &mut editor, "palette-run", &mut clip);
        assert!(
            editor.state.palette,
            "Invalid structured parameters must keep the form open"
        );
        assert!(shell.shown_node("param-error-sources").is_some());
        assert_eq!(editor.host.store().revision(), revision);
        assert!(
            !project.0.join("Assets").exists(),
            "Editing import parameters must not import or stage files"
        );
    }
}

#[test]
fn generated_scalar_numeric_parameter_scrubs_clamps_and_rejects_invalid_text_without_authoring() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_palform1"]}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let mut clip = Clip::default();
    shell.frame().unwrap();
    type_control(
        &mut shell,
        &mut editor,
        "entity-name",
        "Uncommitted Inspector name",
        &mut clip,
    );
    assert_eq!(
        shell.drafts().get("@name").map(String::as_str),
        Some("Uncommitted Inspector name")
    );
    shell.ui.resize([160., 450.]).unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    let revision = editor.host.store().revision();
    open_form(&mut shell, &mut editor, "entity.reparent", &mut clip);
    type_control(&mut shell, &mut editor, "param-index", "2", &mut clip);
    let index = shell.ui.find("param-index").unwrap();
    let node = &shell.ui.nodes[index];
    assert_eq!(
        node.element.attrs.get("step").map(String::as_str),
        Some("1")
    );
    assert_eq!(node.element.attrs.get("min").map(String::as_str), Some("0"));
    let point = [
        node.rect.x + node.rect.w - 8.,
        node.rect.y + node.rect.h * 0.5,
    ];
    shell
        .event(InputEvent::PointerDown(point), &mut clip)
        .unwrap();
    let moved = [point[0] + 24., point[1]];
    shell
        .event(InputEvent::PointerMove(moved), &mut clip)
        .unwrap();
    assert!(shell.input.scrubbing());
    assert_eq!(shell.palette_params["index"].as_u64(), Some(2));
    shell
        .event(InputEvent::PointerUp(moved), &mut clip)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.frame().unwrap();
    assert!(
        shell.palette_params["index"]
            .as_u64()
            .is_some_and(|value| value > 2)
    );
    let rect = shell.ui.nodes[shell.ui.find("param-index").unwrap()].rect;
    let start = [rect.x + rect.w - 8., rect.y + rect.h * 0.5];
    let end = [start[0] - 100_000., start[1]];
    shell
        .event(InputEvent::PointerDown(start), &mut clip)
        .unwrap();
    shell
        .event(InputEvent::PointerMove(end), &mut clip)
        .unwrap();
    shell.event(InputEvent::PointerUp(end), &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.frame().unwrap();
    assert_eq!(
        shell.palette_params["index"].as_u64(),
        Some(0),
        "Numeric scrubbing must honor the schema minimum"
    );
    type_control(&mut shell, &mut editor, "param-index", "-1", &mut clip);
    assert!(!shell.palette_params.contains_key("index"));
    assert!(shell.shown_node("param-error-index").is_some());
    type_control(&mut shell, &mut editor, "param-index", "NaN", &mut clip);
    assert!(!shell.palette_params.contains_key("index"));
    click_control(&mut shell, &mut editor, "palette-run", &mut clip);
    assert_eq!(editor.host.store().revision(), revision);
    assert!(editor.state.palette);
    shell.key(&mut editor, 40, true, &mut clip).unwrap();
    shell.key(&mut editor, 43, true, &mut clip).unwrap();
    assert_eq!(
        editor.host.store().revision(),
        revision,
        "Palette scrub/Enter/Tab must not commit Inspector drafts behind the modal"
    );
    assert_eq!(
        shell.drafts().get("@name").map(String::as_str),
        Some("Uncommitted Inspector name")
    );
    shell.key(&mut editor, 41, true, &mut clip).unwrap();
    assert!(!editor.state.palette);
    assert_eq!(
        shell.drafts().get("@name").map(String::as_str),
        Some("Uncommitted Inspector name")
    );
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
}

#[test]
fn transform_form_focus_and_axis_drafts_survive_rebuild_then_run_one_typed_transaction() {
    for light in [false, true] {
        let project = Project::new();
        let mut editor = Editor::open(&project.0).unwrap();
        editor
            .action("editor.theme", json!({"light":light}))
            .unwrap();
        editor
            .action("editor.select", json!({"entities":["e_palform1"]}))
            .unwrap();
        let mut shell = Shell::new(&mut editor, [160., 450.]).unwrap();
        let mut clip = Clip::default();
        let revision = editor.host.store().revision();
        open_form(&mut shell, &mut editor, "transform.set", &mut clip);
        assert_eq!(shell.input.focused(), Some(stable_id("param-entities")));
        assert_eq!(shell.palette_params["entities"], json!(["e_palform1"]));
        type_control(&mut shell, &mut editor, "param-position-0", "4", &mut clip);
        assert!(
            !shell.palette_params.contains_key("position"),
            "A partial vector must not invent zeroes for the other axes"
        );
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert_eq!(shell.input.focused(), Some(stable_id("param-position-0")));
        let Widget::Input(input) = &shell.ui.widgets[&stable_id("param-position-0")] else {
            panic!("X axis field")
        };
        assert_eq!(input.value(), "4");
        type_control(&mut shell, &mut editor, "param-position-1", "5", &mut clip);
        type_control(&mut shell, &mut editor, "param-position-2", "6", &mut clip);
        assert_eq!(shell.palette_params["position"], json!([4, 5, 6]));
        assert!(
            !shell.palette_params.contains_key("rotation")
                && !shell.palette_params.contains_key("scale")
        );
        click_control(&mut shell, &mut editor, "palette-run", &mut clip);
        assert!(!editor.state.palette);
        assert_eq!(editor.host.store().revision(), revision + 1);
        let position = &editor
            .host
            .store()
            .workspace()
            .doc("sc_palform1")
            .unwrap()
            .record("e_palform1")
            .unwrap()["Transform"]["position"];
        assert_eq!(
            position
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_f64().unwrap())
                .collect::<Vec<_>>(),
            vec![4., 5., 6.]
        );
    }
}

#[test]
fn palette_row_capture_cannot_change_command_across_catalog_generation_or_selection_changes() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let mut clip = Clip::default();
    shell
        .run_command(&mut editor, "edit.palette", &Target::None)
        .unwrap();
    editor
        .action(
            "editor.search",
            json!({"panel":"palette","query":">transform.set"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    let id = result_id(&shell, "transform.set");
    let index = shell.ui.find(&id).unwrap();
    let rect = shell.ui.nodes[index].rect;
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    shell
        .event(InputEvent::PointerDown(point), &mut clip)
        .unwrap();
    assert_eq!(shell.input.captured(), Some(stable_id(&id)));
    editor.extensions.generation = Some("replacement-generation".into());
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert!(shell.input.captured().is_none());
    shell
        .event(InputEvent::PointerUp(point), &mut clip)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(shell.palette_command.is_none());
    let id = result_id(&shell, "transform.set");
    let index = shell.ui.find(&id).unwrap();
    let rect = shell.ui.nodes[index].rect;
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    shell
        .event(InputEvent::PointerDown(point), &mut clip)
        .unwrap();
    editor
        .action("editor.select", json!({"entities":["e_palform1"]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert_ne!(result_id(&shell, "transform.set"), id);
    shell
        .event(InputEvent::PointerUp(point), &mut clip)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(shell.palette_command.is_none());
}
