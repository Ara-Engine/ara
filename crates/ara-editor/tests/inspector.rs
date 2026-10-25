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
        std::fs::create_dir_all(root.join("levels")).unwrap();
        std::fs::create_dir_all(root.join("models")).unwrap();
        write(
            &root.join("ara.project.json"),
            json!({"name":"Inspector test","main":"levels/main.scene"}),
        );
        write(
            &root.join("levels/main.scene"),
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
            std::fs::write(root.join("models").join(file), "{}").unwrap();
            write(
                &root.join(format!("models/{file}.import.json")),
                json!({"$schema":"ara://schema/import/1","id":id,"records":{"i_settings":{"importer":importer,"settings":{}}}}),
            );
        }
        std::fs::write(root.join("models/barrel.gltf"), "{}").unwrap();
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
fn an_asset_field_opens_a_picker_of_the_assets_that_fit_it() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    let mut clip = MemoryClipboard::default();
    shell
        .click("asset-field-MeshRenderer-model", &mut clip)
        .unwrap();
    shell.dispatch(&mut editor).unwrap();
    // Models with an asset ID: not the texture, not the model still importing.
    assert!(menu_row(&shell, "None").is_some());
    assert!(menu_row(&shell, "rock.png").is_none());
    assert!(menu_row(&shell, "barrel.gltf").is_none());
    let row = menu_row(&shell, "crate.gltf").expect("the imported model is offered");
    shell.click(&row, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(model(&editor), "@models/crate.gltf#as_crate0001");
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
    assert!(menu_row(&shell, "crate.gltf").is_none());
    let start = std::time::Instant::now();
    let row = loop {
        if let Some(row) = menu_row(&shell, "steel.graph") {
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
