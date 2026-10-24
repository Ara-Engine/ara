#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! The Inspector's typed editors and every way an asset field is filled: the field's
//! picker, the pick button and Assets, dropping from Assets, and entity picking.
use ara_editor::{Editor, shell::Shell};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// A scene with a crate (a `MeshRenderer`) and a follow camera; in `models/`, an
/// imported model, an imported texture, and a model still importing (no sidecar, so
/// no asset ID yet).
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
                "e_camera01":{"name":"Follow camera","Transform":{},"Camera":{},"FollowCamera":{}}}}),
        );
        for (file, id, importer) in [
            ("crate.gltf", "as_crate0001", "ara.gltf"),
            ("rock.png", "as_rock00001", "ara.image"),
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
        let mut editor = Editor::open(&self.0).unwrap();
        editor
            .action("editor.select", json!({"entities":["e_crate001"]}))
            .unwrap();
        editor
            .action("editor.layout.show", json!({"panel":"assets"}))
            .unwrap();
        editor
            .action("editor.workspace", json!({"folder":"models"}))
            .unwrap();
        let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
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
/// The Assets card showing `name`.
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
/// The crate's `MeshRenderer.model` reference.
fn model(editor: &Editor) -> Value {
    let resolved = ara_editor::world::resolve(editor.host.store(), &editor.state.doc).unwrap();
    resolved["e_crate001"].record["MeshRenderer"]
        .get("model")
        .cloned()
        .unwrap_or(Value::Null)
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
    let card = card(&shell, "crate.gltf");
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
    for (file, why) in [
        ("rock.png", "Model takes a glTF or FBX model, not rock.png"),
        ("barrel.gltf", "barrel.gltf has no asset ID yet"),
    ] {
        let card = card(&shell, file);
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
    shell.pointer = centre(&shell, &card(&shell, "crate.gltf"));
    shell.begin_drag();
    shell.pointer = centre(&shell, "asset-field-MeshRenderer-model");
    assert!(shell.drop_at(&mut editor).unwrap());
    assert_eq!(model(&editor), "@models/crate.gltf#as_crate0001");
    // A texture dropped on the model field is refused, with the reason.
    shell.pointer = centre(&shell, &card(&shell, "rock.png"));
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
    let card = card(&shell, "crate.gltf");
    shell.click(&card, &mut clip).unwrap();
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(
        editor.state.asset_path.as_deref(),
        Some("models/crate.gltf")
    );
    assert_eq!(model(&editor), Value::Null);
}

#[test]
#[ignore = "listing"]
fn list_raw_json_fields() {
    use ara_schema::{Shape, ValueType};
    let project = Project::new();
    let editor = Editor::open(&project.0).unwrap();
    let db = editor.host.store().schema();
    for ty in db.components() {
        let Shape::Struct { fields, .. } = &ty.shape else {
            continue;
        };
        for f in fields.iter().filter(|f| !f.editor.hidden) {
            let inner = if let ValueType::Option(t) = &f.ty {
                t.as_ref()
            } else {
                &f.ty
            };
            let typed = inner.is_number()
                || inner.vector_len().is_some()
                || matches!(
                    inner,
                    ValueType::Bool | ValueType::Asset(_) | ValueType::Entity | ValueType::String
                )
                || matches!(inner, ValueType::Named(n) if matches!(db.get(n).map(|t| &t.shape), Some(Shape::Enum { .. })));
            if !typed {
                println!("RAW {}.{} : {}", ty.name, f.name, f.ty);
            }
        }
    }
}
