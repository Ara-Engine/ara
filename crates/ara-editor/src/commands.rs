//! Editor-owned content commands; all changes go through EditCtx and the host journal.
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_schema::{AssetRef, EntityRef, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) fn register(registry: &mut Registry, db: &mut SchemaDb) {
    crate::fixes::register(registry, db);
    registry
        .command::<Duplicate>(db)
        .command::<Unpack>(db)
        .command::<InstantiateMesh>(db);
}
/// Duplicate a resolved subtree, assigning new stable IDs and preserving its hierarchy.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DuplicateParams {
    /// Root entity address.
    entity: EntityRef,
}
struct Duplicate;
impl Command for Duplicate {
    type Params = DuplicateParams;
    type Output = Value;
    const NAME: &'static str = "editor.entity.duplicate";
    const SUMMARY: &'static str = "Duplicate an entity subtree as an independent copy.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_crate001"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: DuplicateParams) -> Result<Value, Diagnostic> {
        cx.read().target(&p.entity.0)?;
        let resolved = cx.read().resolved();
        let source = descendants(&resolved, &p.entity.0);
        let mut ids = BTreeMap::new();
        for old in &source {
            ids.insert(old.clone(), cx.new_id(None, "e")?);
        }
        for old in &source {
            let entity = &resolved[old];
            let mut record = entity.record.clone();
            let parent = entity.parent.as_ref().map(|p| {
                ids.get(p)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| p.clone())
            });
            if let Some(parent) = parent {
                record.insert("parent".into(), parent.into());
            }
            if let Some(order) = &entity.order {
                record.insert("order".into(), order.clone().into());
            }
            // References inside the copied subtree follow the new IDs.
            remap(cx.read().schema(), &mut record, &ids);
            if old == &p.entity.0 {
                let name = record
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Entity");
                record.insert("name".into(), format!("{name} copy").into());
            }
            cx.create(ids[old].clone(), record)?;
        }
        Ok(json!({"id":ids.get(&p.entity.0),"count":ids.len()}))
    }
}
fn descendants(resolved: &ara_doc::prefab::Resolved, root: &str) -> Vec<String> {
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, entity) in resolved {
        if let Some(parent) = entity.parent.as_deref() {
            children.entry(parent).or_default().push(id);
        }
    }
    let mut out = vec![root.to_owned()];
    let mut i = 0;
    while i < out.len() {
        if let Some(kids) = children.get(out[i].as_str()) {
            out.extend(kids.iter().map(|id| (*id).to_owned()));
        }
        i += 1;
    }
    out
}
fn remap(db: &SchemaDb, record: &mut ara_doc::Record, ids: &BTreeMap<String, ara_doc::RecordId>) {
    fn visit(
        db: &SchemaDb,
        ty: &ara_schema::ValueType,
        value: &mut Value,
        ids: &BTreeMap<String, ara_doc::RecordId>,
    ) {
        use ara_schema::ValueType;
        match (ty, value) {
            (ValueType::Entity, Value::String(id)) => {
                if let Some(mapped) = ids.get(id) {
                    *id = mapped.to_string();
                }
            }
            (ValueType::Option(inner), value) if !value.is_null() => visit(db, inner, value, ids),
            (ValueType::Named(name), Value::Object(object)) => {
                if let Some(desc) = db.get(name) {
                    for field in desc.fields() {
                        if let Some(value) = object.get_mut(&field.name) {
                            visit(db, &field.ty, value, ids);
                        }
                    }
                }
            }
            (ValueType::List(inner) | ValueType::Keyed(inner), Value::Array(values)) => {
                for value in values {
                    visit(db, inner, value, ids);
                }
            }
            (ValueType::Map(inner), Value::Object(values)) => {
                for value in values.values_mut() {
                    visit(db, inner, value, ids);
                }
            }
            _ => {}
        }
    }
    for (component, value) in record
        .iter_mut()
        .filter(|(name, _)| ara_doc::is_component_key(name))
    {
        visit(
            db,
            &ara_schema::ValueType::Named(component.clone()),
            value,
            ids,
        );
    }
}
/// Convert a prefab instance into ordinary records while preserving its resolved values.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnpackParams {
    /// Root prefab instance address.
    entity: EntityRef,
}
struct Unpack;
impl Command for Unpack {
    type Params = UnpackParams;
    type Output = Value;
    const NAME: &'static str = "editor.prefab.unpack";
    const SUMMARY: &'static str =
        "Unpack a prefab instance, preserving stable root identity and undo.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_crate001"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: UnpackParams) -> Result<Value, Diagnostic> {
        let id = ara_doc::RecordId::parse(&p.entity.0).map_err(crate::invalid)?;
        let original = cx
            .read()
            .doc()
            .record(id.as_str())
            .ok_or_else(|| crate::invalid("instance not found"))?;
        if ara_doc::prefab_of(original).is_none() {
            return Err(crate::invalid("select a prefab instance to unpack"));
        }
        let resolved = cx.read().resolved();
        let root = resolved
            .get(&p.entity.0)
            .ok_or_else(|| crate::invalid("prefab root not resolved"))?;
        let mut record = root.record.clone();
        if let Some(parent) = &root.parent {
            record.insert("parent".into(), parent.clone().into());
        }
        if let Some(order) = &root.order {
            record.insert("order".into(), order.clone().into());
        }
        let source: Vec<_> = resolved
            .keys()
            .filter(|addr| addr.starts_with(&format!("{}/", p.entity.0)))
            .cloned()
            .collect();
        let mut ids = BTreeMap::from([(p.entity.0.clone(), id.clone())]);
        for old in &source {
            ids.insert(old.clone(), cx.new_id(None, "e")?);
        }
        remap(cx.read().schema(), &mut record, &ids);
        cx.delete(&id)?;
        cx.create(id, record)?;
        for old in &source {
            let entity = &resolved[old];
            let mut record = entity.record.clone();
            if let Some(parent) = &entity.parent {
                record.insert(
                    "parent".into(),
                    ids.get(parent)
                        .map(ToString::to_string)
                        .unwrap_or_else(|| parent.clone())
                        .into(),
                );
            }
            if let Some(order) = &entity.order {
                record.insert("order".into(), order.clone().into());
            }
            remap(cx.read().schema(), &mut record, &ids);
            cx.create(ids[old].clone(), record)?;
        }
        // Authored children attached inside an instance must follow the unpacked address.
        let attached: Vec<_> = cx
            .read()
            .doc()
            .records()
            .iter()
            .filter_map(|(id, r)| {
                let p = ara_doc::parent_of(r)?;
                ids.get(p).map(|new| (id.clone(), new.to_string()))
            })
            .collect();
        for (id, parent) in attached {
            let order = cx
                .read()
                .doc()
                .record(id.as_str())
                .and_then(ara_doc::order_key);
            cx.move_to(&id, Some(parent), order)?;
        }
        Ok(json!({"entity":p.entity.0,"unpacked":ids.len()}))
    }
}
/// Instantiate a source mesh as one Transform + MeshRenderer entity.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstantiateMeshParams {
    /// Stable mesh reference.
    mesh: AssetRef,
    /// Optional display name; defaults to the source file's name.
    name: Option<String>,
    /// Optional parent.
    parent: Option<EntityRef>,
    /// Placement in parent-local metres.
    #[ara(unit=Meters)]
    position: Option<[f32; 3]>,
}
struct InstantiateMesh;
impl Command for InstantiateMesh {
    type Params = InstantiateMeshParams;
    type Output = Value;
    const NAME: &'static str = "editor.mesh.instantiate";
    const SUMMARY: &'static str = "Place a mesh asset into the scene as one undoable command.";
    fn examples() -> Vec<Value> {
        vec![json!({"mesh":"@meshes/crate.glb#me_crate001"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: InstantiateMeshParams) -> Result<Value, Diagnostic> {
        // Unnamed placements take the source file's name (`@props/rock.glb#…` → rock).
        let name = p.name.unwrap_or_else(|| {
            let path = p.mesh.0.trim_start_matches('@');
            Path::new(path.split('#').next().unwrap_or(path))
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Mesh")
                .to_owned()
        });
        let mut record = ara_doc::Record::new();
        record.insert("name".into(), name.into());
        if let Some(parent) = p.parent {
            cx.read().target(&parent.0)?;
            record.insert("parent".into(), parent.0.into());
        }
        record.insert(
            "Transform".into(),
            json!({"position":p.position.unwrap_or([0.;3])}),
        );
        record.insert("MeshRenderer".into(), json!({"model":p.mesh.0}));
        let id = cx.new_id(None, "e")?;
        cx.create(id.clone(), record)?;
        Ok(json!({"id":id}))
    }
}
/// Resolve a source path under the project; traversal and symlinks may not escape it.
/// The project file `relative` names, which must stay inside the project. A missing
/// file still resolves to where it belongs (callers draw placeholders and report it,
/// `DocumentView::missing_sources`); every error names the path.
pub(crate) fn project_path(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let joined = root.join(relative);
    match std::fs::canonicalize(&joined) {
        Ok(path) => {
            let root = std::fs::canonicalize(root)
                .with_context(|| format!("project folder {}", root.display()))?;
            anyhow::ensure!(
                path.starts_with(root),
                "asset source `{relative}` escapes the project"
            );
            Ok(path)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::ensure!(
                Path::new(relative)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
                "asset source `{relative}` escapes the project"
            );
            Ok(joined)
        }
        Err(error) => Err(anyhow::Error::new(error)
            .context(format!("asset source `{relative}` ({})", joined.display()))),
    }
}

#[cfg(test)]
mod project_path_tests {
    use super::project_path;
    #[test]
    fn missing_files_resolve_inside_the_project_and_errors_name_the_path() {
        let root = std::env::temp_dir();
        let missing = project_path(&root, "assets/kit/no-such-model.gltf")
            .expect("a missing file still has its place in the project");
        assert!(missing.ends_with("assets/kit/no-such-model.gltf"));
        let escape = project_path(&root, "../outside.gltf").expect_err("escapes");
        assert!(escape.to_string().contains("`../outside.gltf`"), "{escape}");
    }
}
