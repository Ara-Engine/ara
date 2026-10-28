#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! The Inspector's typed editors and every way an asset field is filled: the field's
//! picker, the pick button and Assets, dropping from Assets, and entity picking. No
//! field falls back to raw JSON: lists, maps and optional structs edit per element.
use ara_editor::{Editor, shell::Shell};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// A scene with a crate (a `MeshRenderer`), a follow camera and a speaker (audio
/// graph parameters, an acoustic surface); in `models/`, an imported model, an
/// imported texture, and a model still importing (no sidecar, so no asset ID yet).
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ara-editor-inspector-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::create_dir_all(root.join("settings")).unwrap();
        std::fs::write(
            root.join("settings/ara.imports"),
            ara_asset::ProjectImportSettings::default()
                .canonical_text()
                .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("models")).unwrap();
        write(
            &root.join("ara.project.json"),
            json!({"name":"Inspector test","layoutVersion":ara_host::project_layout::VERSION,"main":"levels/main.scene"}),
        );
        write(
            &root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
            json!({"$schema":"ara://schema/scene/1","id":"sc_test0001","records":{
                "e_crate001":{"name":"Salvage crate","Transform":{"position":[0,0.6,0]},"MeshRenderer":{}},
                "e_camera01":{"name":"Follow camera","Transform":{},"Camera":{},"FollowCamera":{}},
                "e_speaker1":{"name":"Speaker","Transform":{},"AudioGraphSource":{},"AcousticSurface":{}}}}),
        );
        for (file, id, importer) in [
            ("crate.gltf", "as_crate0001", "ara.gltf"),
            ("rock.png", "as_rock00001", "ara.image"),
            (
                "exterior_aircon_unit_rusted_with_a_long_name_1k.gltf",
                "as_aircon001",
                "ara.gltf",
            ),
        ] {
            std::fs::write(
                root.join(ara_doc::AUTHORING_ROOT).join("models").join(file),
                "{}",
            )
            .unwrap();
            write(
                &root
                    .join(ara_doc::AUTHORING_ROOT)
                    .join(format!("models/{file}.import.json")),
                json!({"$schema":"ara://schema/import/1","id":id,"records":{"i_settings":{"importer":importer,"settings":{}}}}),
            );
        }
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT)
                .join("models/barrel.gltf"),
            "{}",
        )
        .unwrap();
        Self(root)
    }
    /// The editor with the crate selected and Assets showing `models`, once the asset
    /// browser's first scan has arrived.
    fn open(&self) -> (Editor, Shell) {
        self.open_with(|_| {})
    }
    /// [`Self::open`], running `prepare` on the editor before the shell starts.
    fn open_with(&self, prepare: impl FnOnce(&mut Editor)) -> (Editor, Shell) {
        let mut editor = Editor::open(&self.0).unwrap();
        prepare(&mut editor);
        editor
            .action("editor.select", json!({"entities":["e_crate001"]}))
            .unwrap();
        editor
            .action("editor.layout.show", json!({"panel":"assets"}))
            .unwrap();
        editor
            .action("editor.workspace", json!({"folder":"models"}))
            .unwrap();
        // Hierarchy shares the right column with the Inspector and Assets is open:
        // a tall window keeps every Inspector field the tests click on screen.
        let mut shell = Shell::new(&mut editor, [1440., 2100.]).unwrap();
        let start = std::time::Instant::now();
        while !shell.poll_assets_for_test(&mut editor) {
            assert!(start.elapsed().as_secs() < 10, "asset scan never arrived");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        (editor, shell)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn write(path: &Path, value: Value) {
    std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap();
}
#[derive(Default)]
struct MemoryClipboard(String);
impl ara_ui_text::Clipboard for MemoryClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}
/// The Assets card showing `name` (tiles show names without their extension).
fn card(shell: &Shell, name: &str) -> String {
    shell
        .ui
        .nodes
        .iter()
        .find(|n| {
            n.element.id.starts_with("asset-card-")
                && n.element.id.ends_with("-name")
                && n.text.as_ref().is_some_and(|t| t.value == name)
        })
        .map(|n| n.element.id.trim_end_matches("-name").to_owned())
        .unwrap_or_else(|| panic!("no card for {name}"))
}
/// The root menu row titled `title`.
fn menu_row(shell: &Shell, title: &str) -> Option<String> {
    shell
        .ui
        .nodes
        .iter()
        .find(|n| {
            n.element.id.starts_with("m0-")
                && n.element.id.ends_with("-label")
                && n.text.as_ref().is_some_and(|t| t.value == title)
        })
        .map(|n| n.element.id.trim_end_matches("-label").to_owned())
}
/// The status bar's message.
fn status(shell: &Shell) -> String {
    let i = shell.ui.find("status-saved").unwrap();
    shell.ui.nodes[i].text.as_ref().unwrap().value.clone()
}
/// The centre of the control `id`.
fn centre(shell: &Shell, id: &str) -> [f32; 2] {
    let r = shell.ui.nodes[shell.ui.find(id).unwrap()].rect;
    [r.x + r.w * 0.5, r.y + r.h * 0.5]
}
/// `Component.field` of `entity` as authored (Null when omitted).
fn authored(editor: &Editor, entity: &str, component: &str, field: &str) -> Value {
    let resolved = ara_editor::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    resolved[entity].record[component]
        .get(field)
        .cloned()
        .unwrap_or(Value::Null)
}
/// The crate's `MeshRenderer.model` reference.
fn model(editor: &Editor) -> Value {
    authored(editor, "e_crate001", "MeshRenderer", "model")
}
/// Types `text` into the field `id` and commits it, as Enter does.
fn type_into(shell: &mut Shell, editor: &mut Editor, id: &str, text: &str) {
    use ara_ui::input::InputEvent;
    use ara_ui_text::EditEvent;
    let mut clip = MemoryClipboard::default();
    shell.click(id, &mut clip).unwrap();
    shell
        .event(InputEvent::Edit(EditEvent::SelectAll), &mut clip)
        .unwrap();
    shell
        .event(InputEvent::Edit(EditEvent::Insert(text.into())), &mut clip)
        .unwrap();
    shell.dispatch(editor).unwrap();
    shell.commit_fields(editor).unwrap();
}
/// Clicks `id` and dispatches it.
fn press(shell: &mut Shell, editor: &mut Editor, id: &str) {
    let mut clip = MemoryClipboard::default();
    shell
        .click(id, &mut clip)
        .unwrap_or_else(|e| panic!("{id}: {e}"));
    shell.dispatch(editor).unwrap();
}

#[test]
fn first_selection_uses_the_mounted_inspector_column_width() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 2100.]).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let width = shell.ui.nodes[shell.ui.find("field-Transform-position").unwrap()]
        .rect
        .w;
    shell.rebuild(&mut editor).unwrap();
    let next = shell.ui.nodes[shell.ui.find("field-Transform-position").unwrap()]
        .rect
        .w;
    assert!(
        (width - next).abs() < 1.,
        "column jumped from {width} to {next}"
    );
}

#[test]
fn scalar_scrubs_clamp_preview_and_commit_once_while_units_stay_typed() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let before = editor.host.store().history().len();
    shell.pointer = centre(&shell, "field-MeshRenderer-metallic");
    assert!(shell.begin_scrub(&mut editor).unwrap());
    for dx in [10., 20., 1000.] {
        shell.pointer[0] += dx;
        shell.move_scrub().unwrap();
        assert_eq!(editor.host.store().history().len(), before);
    }
    shell.finish_scrub(&mut editor, false).unwrap();
    assert_eq!(
        authored(&editor, "e_crate001", "MeshRenderer", "metallic"),
        json!(1)
    );
    assert_eq!(editor.host.store().history().len(), before + 1);
    editor.action("history.undo", json!({})).unwrap();
    shell.rebuild(&mut editor).unwrap();
    type_into(
        &mut shell,
        &mut editor,
        "edit-Transform-position-0",
        "150cm",
    );
    assert_eq!(
        authored(&editor, "e_crate001", "Transform", "position")[0],
        json!(1.5)
    );
    let before = editor.host.store().history().len();
    shell.pointer = centre(&shell, "field-MeshRenderer-metallic");
    assert!(shell.begin_scrub(&mut editor).unwrap());
    shell.pointer[0] += 50.;
    shell.move_scrub().unwrap();
    shell.finish_scrub(&mut editor, true).unwrap();
    assert_eq!(editor.host.store().history().len(), before);
}

#[test]
fn multi_edit_only_marks_the_vector_channels_that_differ() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_camera01","path":"Transform.position","value":[3,0.6,0]}})], "Prepare selection", None).unwrap();
    editor
        .action(
            "editor.select",
            json!({"entities":["e_crate001","e_camera01"]}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    for (axis, expected) in [(0, "—"), (1, "0.6"), (2, "0")] {
        let i = shell
            .ui
            .find(&format!("edit-Transform-position-{axis}"))
            .unwrap();
        assert_eq!(shell.ui.nodes[i].text.as_ref().unwrap().value, expected);
    }
    shell.pointer = centre(&shell, "scrub-Transform-position-0");
    shell.begin_scrub(&mut editor).unwrap();
    shell.pointer[0] += 25.;
    shell.move_scrub().unwrap();
    shell.finish_scrub(&mut editor, false).unwrap();
    assert_eq!(
        authored(&editor, "e_crate001", "Transform", "position")[0],
        json!(0.25)
    );
    assert_eq!(
        authored(&editor, "e_camera01", "Transform", "position")[0],
        json!(3.25)
    );
}

#[test]
fn component_values_reset_copy_paste_and_presentation_order_are_independent() {
    use ara_editor::shell::commands::Target;
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let target = Target::Component("Transform".into());
    shell
        .run_command(&mut editor, "component.copy", &target)
        .unwrap();
    assert!(
        shell
            .run_command(
                &mut editor,
                "component.paste",
                &Target::Component("MeshRenderer".into())
            )
            .is_err()
    );
    let before = editor.host.store().history().len();
    shell
        .run_command(&mut editor, "component.reset", &target)
        .unwrap();
    assert_eq!(editor.host.store().history().len(), before + 1);
    assert!(authored(&editor, "e_crate001", "Transform", "position").is_null());
    assert_eq!(
        shell.fields["Transform.position"].provenance,
        ara_editor::inspector::Provenance::Default
    );
    assert!(
        shell.fields["Transform.position"]
            .value
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value.as_f64() == Some(0.))
    );
    shell
        .run_command(&mut editor, "component.paste", &target)
        .unwrap();
    assert_eq!(
        authored(&editor, "e_crate001", "Transform", "position"),
        json!([0, 0.6, 0])
    );
    let before = editor.host.store().history().len();
    shell
        .run_command(&mut editor, "component.move_down", &target)
        .unwrap();
    assert_eq!(editor.host.store().history().len(), before);
    let transform = shell.ui.nodes[shell.ui.find("component-Transform").unwrap()].rect;
    let mesh = shell.ui.nodes[shell.ui.find("component-MeshRenderer").unwrap()].rect;
    assert!(mesh.y < transform.y);
    press(&mut shell, &mut editor, "collapse-component-MeshRenderer");
    assert!(shell.ui.find("edit-MeshRenderer-metallic").is_none());
    // Search reveals matching fields even inside remembered collapsed sections.
    editor
        .action(
            "editor.search",
            json!({"panel":"inspector","query":"metallic"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find("edit-MeshRenderer-metallic").is_some());
    editor
        .action("editor.search", json!({"panel":"inspector","query":""}))
        .unwrap();
    drop(shell);
    let mut shell = Shell::new(&mut editor, [1440., 2100.]).unwrap();
    assert!(shell.ui.find("edit-MeshRenderer-metallic").is_none());
    press(&mut shell, &mut editor, "collapse-component-MeshRenderer");
    assert!(shell.ui.find("edit-MeshRenderer-metallic").is_some());
}

#[test]
fn header_enable_is_a_reflected_field_edit_and_optional_assets_use_override_rows() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor
        .apply(
            vec![json!({"cmd":"component.add","args":{"entity":"e_crate001","component":"Brain"}})],
            "Add brain",
            None,
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find("component-enabled-Transform").is_none());
    assert!(shell.ui.find("component-enabled-Brain").is_some());
    assert!(shell.ui.find("edit-Brain-enabled").is_none());
    let before = editor.host.store().history().len();
    press(&mut shell, &mut editor, "component-enabled-Brain");
    assert_eq!(
        authored(&editor, "e_crate001", "Brain", "enabled"),
        json!(false)
    );
    assert_eq!(editor.host.store().history().len(), before + 1);
    assert!(shell.ui.find("override-Brain-behavior").is_some());
}

/// The text of the node `id`.
fn text(shell: &Shell, id: &str) -> String {
    let i = shell.ui.find(id).unwrap_or_else(|| panic!("no {id}"));
    shell.ui.nodes[i]
        .text
        .as_ref()
        .map(|t| t.value.clone())
        .unwrap_or_default()
}
/// A stored colour's channels as numbers (documents may write `1` or `1.0`).
fn channels(value: Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not a colour: {value}"))
        .iter()
        .map(|c| c.as_f64().unwrap())
        .collect()
}
fn history(editor: &Editor) -> usize {
    editor.host.store().history().len()
}
fn picker_shown(shell: &Shell) -> bool {
    shell
        .ui
        .find("color-picker")
        .is_some_and(|i| shell.ui.shown(i))
}

#[test]
fn the_colour_picker_previews_drags_applies_each_gesture_once_and_escape_restores() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    press(&mut shell, &mut editor, "swatch-MeshRenderer-color");
    let rect = shell.ui.nodes[shell.ui.find("color-picker").unwrap()].rect;
    assert!(rect.w > 0. && rect.h > 0. && rect.x >= 0. && rect.y >= 0.);
    assert!(rect.x + rect.w <= shell.ui.size[0] + 0.5 && rect.y + rect.h <= shell.ui.size[1] + 0.5);
    // Typed hex is exact sRGB and one transaction.
    let before = history(&editor);
    type_into(&mut shell, &mut editor, "cp-hex", "#FF0000");
    assert_eq!(
        channels(authored(&editor, "e_crate001", "MeshRenderer", "color")),
        vec![1.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(history(&editor), before + 1);
    type_into(&mut shell, &mut editor, "cp-hex", "#808080");
    // Exact sRGB, not gamma 2.2 (0.2158, not 0.2176).
    assert_eq!(
        authored(&editor, "e_crate001", "MeshRenderer", "color")[0].as_f64(),
        Some(0.21586)
    );
    // Dragging around the ring previews without authoring and applies once, on release.
    type_into(&mut shell, &mut editor, "cp-hex", "#FF0000");
    let before = history(&editor);
    let wheel = shell.ui.nodes[shell.ui.find("cp-wheel").unwrap()].rect;
    let (cx, cy, radius) = (
        wheel.x + wheel.w / 2.,
        wheel.y + wheel.h / 2.,
        wheel.w / 2. - 8.,
    );
    shell.pointer = [cx + radius, cy];
    assert!(shell.begin_picker_drag(&mut editor).unwrap());
    for angle in [0.5f32, 1.5, std::f32::consts::PI] {
        shell.pointer = [cx + radius * angle.cos(), cy - radius * angle.sin()];
        assert!(shell.move_picker().unwrap());
        assert_eq!(history(&editor), before);
    }
    assert!(shell.finish_picker(&mut editor).unwrap());
    assert_eq!(history(&editor), before + 1);
    let cyan = authored(&editor, "e_crate001", "MeshRenderer", "color");
    assert!(
        cyan[0].as_f64().unwrap() < 0.01 && cyan[1].as_f64().unwrap() > 0.99,
        "{cyan}"
    );
    // Escape restores the colour it opened with (the default white) as one more
    // transaction, leaving no authored value behind.
    assert!(shell.picker_escape(&mut editor).unwrap());
    shell.rebuild(&mut editor).unwrap();
    assert!(!picker_shown(&shell));
    assert!(authored(&editor, "e_crate001", "MeshRenderer", "color").is_null());
    assert_eq!(history(&editor), before + 2);
    assert_eq!(text(&shell, "edit-MeshRenderer-color"), "#FFFFFF");
}

#[test]
fn the_colour_picker_closes_on_click_away_unless_pinned_and_keeps_swatches() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    press(&mut shell, &mut editor, "swatch-MeshRenderer-color");
    type_into(&mut shell, &mut editor, "cp-hex", "#3366CC");
    // Saving a swatch writes the project's colour list; Recent fills when it closes.
    press(&mut shell, &mut editor, "cp-add-swatch");
    assert_eq!(
        editor.state.settings["color.swatches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let away = centre(&shell, "entity-name");
    assert!(shell.dismiss_popovers(&mut editor, away).unwrap());
    assert!(!picker_shown(&shell));
    assert_eq!(
        editor.state.settings["color.recent"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // A swatch applies in one transaction.
    press(&mut shell, &mut editor, "swatch-MeshRenderer-color");
    type_into(&mut shell, &mut editor, "cp-hex", "#000000");
    let before = history(&editor);
    press(&mut shell, &mut editor, "cp-swatch-0");
    assert_eq!(history(&editor), before + 1);
    assert_eq!(text(&shell, "cp-hex"), "3366CC");
    // Pinned, it stays open through clicks outside it.
    press(&mut shell, &mut editor, "cp-pin");
    assert!(!shell.dismiss_popovers(&mut editor, away).unwrap());
    assert!(picker_shown(&shell));
}

#[test]
fn hdr_colours_edit_a_base_colour_and_an_intensity_in_stops() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    press(&mut shell, &mut editor, "swatch-MeshRenderer-emissive");
    assert!(shell.ui.find("cp-stops-track").is_some());
    assert!(shell.ui.find("cp-kelvin-track").is_none());
    type_into(&mut shell, &mut editor, "cp-hex", "#FF0000");
    type_into(&mut shell, &mut editor, "cp-stops", "2");
    assert_eq!(
        channels(authored(&editor, "e_crate001", "MeshRenderer", "emissive")),
        vec![4.0, 0.0, 0.0]
    );
    press(&mut shell, &mut editor, "cp-exposure-3");
    assert_eq!(
        channels(authored(&editor, "e_crate001", "MeshRenderer", "emissive")),
        vec![8.0, 0.0, 0.0]
    );
    // The Inspector shows the base colour's hex and the intensity; typed hex keeps it.
    shell.close_color_picker(&mut editor, false).unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(text(&shell, "edit-MeshRenderer-emissive"), "#FF0000");
    assert_eq!(text(&shell, "stops-MeshRenderer-emissive"), "+3.0 EV");
    type_into(
        &mut shell,
        &mut editor,
        "edit-MeshRenderer-emissive",
        "#00FF00",
    );
    assert_eq!(
        channels(authored(&editor, "e_crate001", "MeshRenderer", "emissive")),
        vec![0.0, 8.0, 0.0]
    );
}

/// The Add Component row labelled `name`, when the dropdown shows it.
fn add_row(shell: &Shell, name: &str) -> Option<String> {
    shell
        .ui
        .nodes
        .iter()
        .find(|n| {
            n.element.id.starts_with("add-component-")
                && n.element.id.ends_with("-label")
                && n.text.as_ref().is_some_and(|t| t.value == name)
        })
        .map(|n| n.element.id.trim_end_matches("-label").to_owned())
}

#[test]
fn add_component_is_a_searchable_dropdown_that_adds_in_one_step() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    press(&mut shell, &mut editor, "add-component");
    let pop = shell.ui.find("add-component-pop").unwrap();
    assert!(shell.ui.shown(pop));
    // Components the selection already has are not offered; New Script is.
    assert!(add_row(&shell, "Mesh renderer").is_none());
    assert!(add_row(&shell, "New Script…").is_some());
    type_into(&mut shell, &mut editor, "add-component-filter", "ligh");
    let row = add_row(&shell, "Light").expect("Light matches the search");
    assert!(add_row(&shell, "Camera").is_none());
    let before = history(&editor);
    press(&mut shell, &mut editor, &row);
    assert_eq!(history(&editor), before + 1);
    let resolved = ara_editor::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert!(resolved["e_crate001"].record.contains_key("Light"));
    assert!(
        shell
            .ui
            .find("add-component-pop")
            .is_none_or(|i| !shell.ui.shown(i))
    );
}

/// Until the asset pipeline first reports, a project with imports says they are being
/// prepared rather than showing nothing (the loading window shows the same stage).
#[test]
fn the_status_bar_says_imports_are_preparing_before_the_pipeline_reports() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    let chip = shell.ui.find("status-imports-text").expect("import chip");
    assert_eq!(
        shell.ui.nodes[chip].text.as_ref().map(|t| t.value.as_str()),
        Some("Preparing imports…")
    );
}

/// Revealing a field (diagnostics, agents, `editor.focus`) scrolls the Inspector to
/// it, also when an earlier reveal already scrolled it.
#[test]
fn revealing_a_field_scrolls_the_inspector_to_it_from_anywhere() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.select", json!({"entities":["e_crate001"]}))
        .unwrap();
    // A short window: the Inspector shows a few rows at a time.
    let mut shell = Shell::new(&mut editor, [1440., 640.]).unwrap();
    for path in ["MeshRenderer.metallic", "MeshRenderer.size"] {
        editor
            .action("editor.focus", json!({"panel":"inspector","field":path}))
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
        let rect = |id: &str| shell.ui.nodes[shell.ui.find(id).unwrap()].rect;
        let (view, row) = (
            rect("inspector-scroll"),
            rect(&format!("field-{}", path.replace('.', "-"))),
        );
        assert!(
            row.y >= view.y - 0.5 && row.y + row.h <= view.y + view.h + 0.5,
            "{path} at {row:?} is outside the Inspector's view {view:?}"
        );
    }
}

#[test]
fn an_asset_field_opens_a_picker_of_the_assets_that_fit_it() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let mut clip = MemoryClipboard::default();
    shell
        .click("asset-field-MeshRenderer-model", &mut clip)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    // Models with an asset ID: not the texture, not the model still importing. Rows
    // name assets without their extension, like Assets.
    assert!(menu_row(&shell, "None").is_some());
    assert!(menu_row(&shell, "rock").is_none());
    assert!(menu_row(&shell, "barrel").is_none());
    assert!(menu_row(&shell, "crate.gltf").is_none());
    let row = menu_row(&shell, "crate").expect("the imported model is offered");
    shell.click(&row, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(model(&editor), "@models/crate.gltf#as_crate0001");
    shell.rebuild(&mut editor).unwrap();
    let field = shell.ui.find("asset-name-MeshRenderer-model").unwrap();
    assert_eq!(
        shell.ui.nodes[field]
            .text
            .as_ref()
            .map(|t| t.value.as_str()),
        Some("crate")
    );
    // None clears it.
    shell
        .click("asset-field-MeshRenderer-model", &mut clip)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    let none = menu_row(&shell, "None").unwrap();
    shell.click(&none, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(model(&editor), Value::Null);
}

#[test]
fn the_pick_button_fills_an_asset_field_from_assets() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let mut clip = MemoryClipboard::default();
    shell.click("pick-MeshRenderer-model", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    // Picking shows under the field until an asset is picked.
    assert!(shell.ui.find("picking-MeshRenderer-model").is_some());
    let card = card(&shell, "crate");
    shell.click(&card, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(model(&editor), "@models/crate.gltf#as_crate0001");
    assert!(shell.ui.find("picking-MeshRenderer-model").is_none());
}

#[test]
fn picks_that_cannot_fill_a_field_say_why_and_picking_goes_on() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let mut clip = MemoryClipboard::default();
    shell.click("pick-MeshRenderer-model", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    for (name, why) in [
        ("rock", "Model takes a glTF or FBX model, not rock.png"),
        ("barrel", "barrel.gltf has no asset ID yet"),
    ] {
        let card = card(&shell, name);
        shell.click(&card, &mut clip).unwrap();
        shell.dispatch(&mut editor).unwrap();
        assert_eq!(model(&editor), Value::Null);
        assert!(status(&shell).contains(why), "{}", status(&shell));
        assert!(shell.ui.find("picking-MeshRenderer-model").is_some());
    }
    // Escape stops picking and dismisses the message.
    shell.key(&mut editor, 41, true, &mut clip).unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.ui.find("picking-MeshRenderer-model").is_none());
    assert!(!status(&shell).contains("barrel"));
}

#[test]
fn dropping_an_asset_from_assets_fills_the_field() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    shell.pointer = centre(&shell, &card(&shell, "crate"));
    shell.begin_drag();
    shell.pointer = centre(&shell, "asset-field-MeshRenderer-model");
    assert!(shell.drop_at(&mut editor).unwrap());
    assert_eq!(model(&editor), "@models/crate.gltf#as_crate0001");
    // A texture dropped on the model field is refused, with the reason.
    shell.pointer = centre(&shell, &card(&shell, "rock"));
    shell.begin_drag();
    shell.pointer = centre(&shell, "asset-field-MeshRenderer-model");
    assert!(shell.drop_at(&mut editor).is_err());
    assert_eq!(model(&editor), "@models/crate.gltf#as_crate0001");
    shell.rebuild(&mut editor).unwrap();
    assert!(
        status(&shell).contains("not rock.png"),
        "{}",
        status(&shell)
    );
}

#[test]
fn entity_fields_pick_from_the_hierarchy() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor
        .action("editor.select", json!({"entities":["e_camera01"]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let mut clip = MemoryClipboard::default();
    shell.click("pick-FollowCamera-target", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.click_entity("e_crate001", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    let resolved = ara_editor::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert_eq!(
        resolved["e_camera01"].record["FollowCamera"]["target"],
        "e_crate001"
    );
    // Picking does not change the selection.
    assert_eq!(editor.state.selection, ["e_camera01"]);
    shell.click("clear-FollowCamera-target", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    let resolved = ara_editor::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert!(resolved["e_camera01"].record["FollowCamera"]["target"].is_null());
    editor.action("history.undo", json!({})).unwrap();
    let resolved = ara_editor::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    assert_eq!(
        resolved["e_camera01"].record["FollowCamera"]["target"],
        "e_crate001"
    );
}

#[test]
fn clicking_an_asset_in_assets_selects_it() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let mut clip = MemoryClipboard::default();
    let card = card(&shell, "crate");
    shell.click(&card, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(
        editor.state.asset_path.as_deref(),
        Some("models/crate.gltf")
    );
    assert_eq!(model(&editor), Value::Null);
}

/// Whether the Inspector edits `ty` with typed widgets all the way down: no element
/// of any list, map or struct falls back to a JSON text box.
fn typed(db: &ara_schema::SchemaDb, ty: &ara_schema::ValueType) -> bool {
    use ara_schema::{Shape, ValueType};
    match ty {
        ValueType::Json => false,
        ValueType::Option(t) | ValueType::List(t) | ValueType::Keyed(t) | ValueType::Map(t) => {
            typed(db, t)
        }
        ValueType::Named(name) => match db.get(name).map(|t| &t.shape) {
            Some(Shape::Struct { fields, .. }) => fields.iter().all(|f| typed(db, &f.ty)),
            Some(Shape::Enum { .. }) => true,
            _ => false,
        },
        _ => true,
    }
}

#[test]
fn no_inspector_field_falls_back_to_raw_json() {
    use ara_editor::inspector::{Widget, widget};
    use ara_schema::{Shape, ValueType};
    let project = Project::new();
    let editor = Editor::open(&project.0).unwrap();
    let db = editor.host.store().schema();
    let mut raw = Vec::new();
    let mut compound = 0;
    for ty in db.components() {
        let Shape::Struct { fields, .. } = &ty.shape else {
            continue;
        };
        for f in fields.iter().filter(|f| !f.editor.hidden) {
            let text_ok = matches!(&f.ty, ValueType::String)
                || matches!(&f.ty, ValueType::Option(t) if **t == ValueType::String);
            match widget(db, f) {
                Widget::Text if !text_ok => raw.push(format!("{}.{}: {}", ty.name, f.name, f.ty)),
                Widget::Compound { .. } => compound += 1,
                _ => {}
            }
            if !typed(db, &f.ty) {
                raw.push(format!("{}.{}: {} (an element)", ty.name, f.name, f.ty));
            }
        }
    }
    assert!(raw.is_empty(), "raw JSON fields: {raw:#?}");
    // The fifteen list/map/struct fields this replaced, among them Material Overrides.
    assert!(compound >= 15, "{compound} compound fields");
}

#[test]
fn material_overrides_add_slots_by_picking_a_material() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open_with(|editor| {
        editor
            .action(
                "editor.resource.create",
                json!({"path":"materials/steel.graph","domain":"material"}),
            )
            .unwrap();
        editor.host.flush();
    });
    let start = std::time::Instant::now();
    while !shell
        .poll_assets_for_test(&mut editor)
        .then(|| shell.ui.find("add-MeshRenderer-materialOverrides"))
        .flatten()
        .is_some()
    {
        assert!(start.elapsed().as_secs() < 10, "no Material overrides row");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut clip = MemoryClipboard::default();
    // Real widgets for the material fields around the slots.
    assert!(shell.ui.find("swatch-MeshRenderer-emissive").is_some());
    assert!(shell.ui.find("slider-MeshRenderer-roughness").is_some());
    assert!(shell.ui.find("slider-MeshRenderer-metallic").is_some());
    // Add opens a picker of materials; picking one adds the slot (no empty slot).
    shell
        .click("add-MeshRenderer-materialOverrides", &mut clip)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert!(menu_row(&shell, "None").is_none());
    assert!(menu_row(&shell, "crate").is_none());
    let start = std::time::Instant::now();
    let row = loop {
        if let Some(row) = menu_row(&shell, "steel") {
            break row;
        }
        assert!(
            start.elapsed().as_secs() < 10,
            "the material is not offered"
        );
        shell.poll_assets_for_test(&mut editor);
        shell
            .click("add-MeshRenderer-materialOverrides", &mut clip)
            .unwrap();
        shell.dispatch(&mut editor).unwrap();
    };
    shell.click(&row, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    let overrides = authored(&editor, "e_crate001", "MeshRenderer", "materialOverrides");
    let steel = overrides[0]["material"].as_str().unwrap().to_owned();
    assert!(steel.starts_with("@materials/steel.graph#"), "{overrides}");
    shell.rebuild(&mut editor).unwrap();
    assert!(
        shell
            .ui
            .find("asset-field-MeshRenderer-materialOverrides-0-material")
            .is_some()
    );
    // The slot's primitive is a number field.
    type_into(
        &mut shell,
        &mut editor,
        "nest-edit-MeshRenderer-materialOverrides-0-primitive",
        "2",
    );
    assert_eq!(
        authored(&editor, "e_crate001", "MeshRenderer", "materialOverrides"),
        json!([{"primitive": 2, "material": steel}])
    );
    // Remove drops the slot.
    press(
        &mut shell,
        &mut editor,
        "remove-MeshRenderer-materialOverrides-0",
    );
    assert_eq!(
        authored(&editor, "e_crate001", "MeshRenderer", "materialOverrides"),
        json!([])
    );
}

#[test]
fn maps_and_optional_structs_edit_per_entry() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor
        .action("editor.select", json!({"entities":["e_speaker1"]}))
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    // A map entry: add, rename its key, set its value, remove it.
    press(&mut shell, &mut editor, "add-AudioGraphSource-parameters");
    let parameters = || authored(&editor, "e_speaker1", "AudioGraphSource", "parameters");
    assert_eq!(parameters(), json!({"parameter1": 0}));
    shell.rebuild(&mut editor).unwrap();
    type_into(
        &mut shell,
        &mut editor,
        "nest-key-AudioGraphSource-parameters-parameter1",
        "volume",
    );
    type_into(
        &mut shell,
        &mut editor,
        "nest-edit-AudioGraphSource-parameters-volume",
        "0.5",
    );
    assert_eq!(
        authored(&editor, "e_speaker1", "AudioGraphSource", "parameters"),
        json!({"volume": 0.5})
    );
    press(
        &mut shell,
        &mut editor,
        "remove-AudioGraphSource-parameters-volume",
    );
    assert_eq!(
        authored(&editor, "e_speaker1", "AudioGraphSource", "parameters"),
        json!({})
    );
    // An optional struct: None until added, then its fields; Remove clears it.
    let custom = |editor: &Editor| authored(editor, "e_speaker1", "AcousticSurface", "custom");
    assert_eq!(custom(&editor), Value::Null);
    press(&mut shell, &mut editor, "add-AcousticSurface-custom");
    assert_eq!(custom(&editor)["scattering"], json!(0.05));
    shell.rebuild(&mut editor).unwrap();
    assert!(
        shell
            .ui
            .find("nest-slider-AcousticSurface-custom-scattering")
            .is_some()
    );
    type_into(
        &mut shell,
        &mut editor,
        "nest-edit-AcousticSurface-custom-scattering",
        "0.25",
    );
    assert_eq!(custom(&editor)["scattering"], json!(0.25));
    let before = editor.host.store().history().len();
    shell.pointer = centre(&shell, "scrub-nest-AcousticSurface-custom-scattering");
    assert!(shell.begin_scrub(&mut editor).unwrap());
    shell.pointer[0] += 25.;
    shell.move_scrub().unwrap();
    assert_eq!(editor.host.store().history().len(), before);
    shell.finish_scrub(&mut editor, false).unwrap();
    assert_eq!(editor.host.store().history().len(), before + 1);
    assert!(custom(&editor)["scattering"].as_f64().unwrap() > 0.25);
    editor.action("history.undo", json!({})).unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(custom(&editor)["scattering"], json!(0.25));
    // Each absorption band is its own number box.
    type_into(
        &mut shell,
        &mut editor,
        "nest-edit-AcousticSurface-custom-absorption-1",
        "0.4",
    );
    assert_eq!(custom(&editor)["absorption"][1], json!(0.4));
    press(&mut shell, &mut editor, "remove-AcousticSurface-custom");
    assert_eq!(custom(&editor), Value::Null);
}

#[test]
fn long_asset_names_clip_and_keep_the_field_buttons_in_view() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor
        .apply(
            vec![json!({"cmd":"property.set","args":{"entity":"e_crate001","path":"MeshRenderer.model",
                "value":"@models/exterior_aircon_unit_rusted_with_a_long_name_1k.gltf#as_aircon001"}})],
            "Long model",
            None,
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    let rect = |id: &str| shell.ui.nodes[shell.ui.find(id).unwrap()].rect;
    let panel = rect("inspector-scroll");
    for id in ["pick-MeshRenderer-model", "clear-MeshRenderer-model"] {
        let r = rect(id);
        assert!(
            r.x + r.w <= panel.x + panel.w + 0.5,
            "{id} overflows: {r:?} vs {panel:?}"
        );
    }
}

/// A model replaces the procedural shape (`showIf` hints): picking one hides Shape and
/// Size and shows Node and Primitive; clearing it brings the shape back. A hidden
/// field keeps its value and stays settable through commands.
#[test]
fn a_model_hides_the_shape_fields_and_clearing_it_brings_them_back() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let shown = |shell: &Shell, field: &str| {
        shell
            .ui
            .find(&format!("field-MeshRenderer-{field}"))
            .is_some()
    };
    let rows = |shell: &Shell| ["shape", "size", "node", "primitive"].map(|f| shown(shell, f));
    assert_eq!(rows(&shell), [true, true, false, false]);
    let mut clip = MemoryClipboard::default();
    shell.click("pick-MeshRenderer-model", &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    let card = card(&shell, "crate");
    shell.click(&card, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(rows(&shell), [false, false, true, true]);
    editor
        .apply(
            vec![json!({"cmd":"property.set","args":{"entity":"e_crate001","path":"MeshRenderer.size","value":[2,2,2]}})],
            "Size while hidden",
            None,
        )
        .unwrap();
    press(&mut shell, &mut editor, "clear-MeshRenderer-model");
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(model(&editor), Value::Null);
    assert_eq!(rows(&shell), [true, true, false, false]);
    assert_eq!(
        authored(&editor, "e_crate001", "MeshRenderer", "size"),
        json!([2, 2, 2])
    );
}
