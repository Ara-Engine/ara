//! Asset creation, population, reference repair and scene replacement share one transaction.
mod references;
use super::*;
use ara_doc::{Edit, field, prefab_of};

/// Parameters for creating a reusable prefab from an authored subtree.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrefabCreateParams {
    /// Root record to replace, preserving its ID, parent, order and local pose.
    pub entity: EntityRef,
    /// New project-relative .prefab path.
    pub path: String,
    /// Optional stable asset identity or batch symbol.
    pub id: Option<String>,
}

/// Atomically convert a subtree to a linked prefab, or an instance to a linked variant.
pub struct PrefabCreate;
impl Command for PrefabCreate {
    type Params = PrefabCreateParams;
    type Output = Value;
    const NAME: &'static str = "prefab.create";
    const SUMMARY: &'static str =
        "Create a prefab asset and replace its source subtree in one undoable transaction.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_8b21","path":"prefabs/crate.prefab"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: PrefabCreateParams) -> Result<Value, Diagnostic> {
        create(ctx, params, false)
    }
}

/// Create an inherited prefab variant from an existing instance without flattening its base.
pub struct PrefabVariant;
impl Command for PrefabVariant {
    type Params = PrefabCreateParams;
    type Output = Value;
    const NAME: &'static str = "prefab.variant";
    const SUMMARY: &'static str =
        "Create a linked prefab variant while preserving base inheritance and instance placement.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_4f9a","path":"prefabs/crate-red.prefab"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: PrefabCreateParams) -> Result<Value, Diagnostic> {
        create(ctx, params, true)
    }
}

fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message)
}

fn create(
    ctx: &mut EditCtx<'_, '_>,
    params: PrefabCreateParams,
    require_variant: bool,
) -> Result<Value, Diagnostic> {
    let target = ctx.read().target(&params.entity.0)?;
    let root = match target {
        Target::Plain(root)
        | Target::Prefab {
            instance: root,
            inner: None,
        } => root,
        _ => {
            return Err(invalid(
                "Open this nested instance's owning prefab to create its variant there",
            ));
        }
    };
    let read = ctx.read();
    let doc = read.doc().id.clone();
    let original = read
        .doc()
        .record(root.as_str())
        .cloned()
        .ok_or_else(|| invalid("The selected root no longer exists"))?;
    let variant = prefab_of(&original).is_some();
    if require_variant && !variant {
        return Err(invalid(
            "Select a prefab instance to create an inherited variant",
        ));
    }
    let subtree = read.doc().subtree(&root);
    let records: Vec<_> = subtree
        .iter()
        .map(|id| {
            (
                id.clone(),
                read.doc().record(id.as_str()).cloned().unwrap_or_default(),
            )
        })
        .collect();
    let resolved = read.resolved();
    let effective = resolved
        .get(root.as_str())
        .ok_or_else(|| invalid("The selected entity cannot be resolved"))?;
    let transform = effective.record.get("Transform");
    let has_transform = transform.is_some();
    let position = transform
        .and_then(|value| value.get("position"))
        .cloned()
        .unwrap_or_else(|| json!([0, 0, 0]));
    let rotation = transform
        .and_then(|value| value.get("rotation"))
        .cloned()
        .unwrap_or_else(|| json!([0, 0, 0]));
    let selected: std::collections::BTreeSet<_> = subtree.iter().map(|id| id.as_str()).collect();
    let addresses: Vec<_> = resolved
        .keys()
        .filter(|address| selected.contains(address.split('/').next().unwrap_or(address)))
        .filter(|address| address.as_str() != root.as_str())
        .cloned()
        .collect();
    let mut ids = std::collections::BTreeMap::new();
    for id in &subtree {
        ids.insert(id.to_string(), ctx.new_id(None, "e")?);
    }
    let source_ids: std::collections::BTreeMap<_, _> = ids
        .iter()
        .map(|(old, new)| (old.clone(), new.to_string()))
        .collect();
    let remap: std::collections::BTreeMap<String, String> = addresses
        .into_iter()
        .map(|address| {
            let source =
                references::replacement(&address, &source_ids).unwrap_or_else(|| address.clone());
            (address, format!("{root}/{source}"))
        })
        .collect();
    let source = ctx.create_document(&params.path, "prefab", params.id.as_deref())?;
    let mut overrides = Map::new();
    for (id, mut record) in records.iter().rev().cloned() {
        if id == root {
            record.shift_remove(field::PARENT);
            record.shift_remove(field::ORDER);
            if variant {
                if let Some(overrides) = record
                    .get_mut(field::OVERRIDES)
                    .and_then(Value::as_object_mut)
                {
                    overrides.shift_remove("Transform.position");
                    overrides.shift_remove("Transform.rotation");
                }
            } else if let Some(transform) =
                record.get_mut("Transform").and_then(Value::as_object_mut)
            {
                transform.shift_remove("position");
                transform.shift_remove("rotation");
            }
        }
        let copied = ids
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| invalid("A copied record lost its new identity"))?;
        references::rewrite_record(
            ctx.read().schema(),
            &mut record,
            &source_ids,
            &remap,
            copied.as_str(),
            id == root,
            &mut overrides,
        );
        ctx.apply_edit(Edit::Create {
            doc: source.clone(),
            id: copied,
            record,
        })?;
    }
    references::remap_scene(ctx, &selected, &remap)?;
    for id in &subtree {
        ctx.delete(id)?;
    }
    let mut instance = Record::new();
    for key in [field::NAME, field::PARENT, field::ORDER] {
        if let Some(value) = original.get(key) {
            instance.insert(key.into(), value.clone());
        }
    }
    instance.insert(
        field::PREFAB.into(),
        json!(format!("@{}#{}", params.path, source)),
    );
    if has_transform {
        overrides.insert("Transform.position".into(), position);
        overrides.insert("Transform.rotation".into(), rotation);
    }
    if !overrides.is_empty() {
        instance.insert(field::OVERRIDES.into(), Value::Object(overrides));
    }
    ctx.apply_edit(Edit::Create {
        doc,
        id: root.clone(),
        record: instance,
    })?;
    Ok(
        json!({"id":root,"entity":root,"doc":source,"path":params.path,"variant":variant,"remappedReferences":remap.len()}),
    )
}
