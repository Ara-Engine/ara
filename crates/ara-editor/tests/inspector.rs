#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.
//! The Inspector's typed editors and every way an asset field is filled: the field's
//! picker, the pick button and Assets, dropping from Assets, and entity picking. No
//! field falls back to raw JSON: lists, maps and optional structs edit per element.
use ara_editor::Editor;
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





