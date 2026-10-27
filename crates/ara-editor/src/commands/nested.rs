//! Execution-time patches preserve each selected object's unrelated collection data.
mod address;
pub(crate) use address::hints;
#[cfg(test)]
mod tests;

use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_schema::{EntityRef, FieldPath, Schema, SchemaDb, ValueType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Stable interpretation of an array token captured by the editor's collection view.
#[derive(Clone, Debug, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "editor", name = "EditorNestedAddressHint")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AddressHint {
    /// Zero-based JSON-pointer token depth.
    pub depth: u32,
    /// A keyed item's identity; resolves independently in each object's collection.
    pub key: Option<String>,
    /// The source pointer was the append position; use each object's current length.
    pub append: bool,
}

/// One typed nested authoring operation, applied to current authoritative state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "editor", name = "EditorNestedOperation")]
#[serde(rename_all = "camelCase")]
pub(crate) enum Operation {
    /// Set a scalar, reference, optional override or explicit whole subtree.
    Set,
    /// Append a list item or a fresh map entry.
    Append,
    /// Remove the addressed list item or map entry.
    Remove,
    /// Move an array item to an explicit index.
    Move,
    /// Move an array item one position earlier in its own collection.
    MoveUp,
    /// Move an array item one position later in its own collection.
    MoveDown,
    /// Rename a map key without changing its per-object value.
    Rename,
    /// Set three linear colour channels while preserving each object's alpha.
    Hex,
}

/// Parameters for a nested field patch; the command returns an explicit missing-path no-op.
#[derive(Schema, Deserialize)]
#[ara(params, module = "editor", name = "EditorNestedEditParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Params {
    /// Target entity, including resolved prefab addresses.
    entity: EntityRef,
    /// Reflected root field containing the nested value.
    path: String,
    /// JSON pointer inside the field; an empty pointer names the field itself.
    pointer: String,
    /// Keyed identities and append positions captured from the visible collection.
    hints: Option<Vec<AddressHint>>,
    /// Typed authoring operation.
    operation: Operation,
    /// New value, template, map-entry pair, key text or RGB channels for the operation.
    value: Value,
    /// Destination index for a Move operation.
    index: Option<u32>,
}

struct Edit;
impl Command for Edit {
    type Params = Params;
    type Output = Value;
    const NAME: &'static str = "editor.property.nested.edit";
    const SUMMARY: &'static str =
        "Edit one nested value on each object's current collection, preserving unrelated data.";

    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_crate001","path":"Transform.position","pointer":"/0","operation":"set","value":2.}),
        ]
    }

    fn run(cx: &mut EditCtx<'_, '_>, params: Params) -> Result<Value, Diagnostic> {
        let read = cx.read();
        let path: FieldPath = params.path.parse().map_err(crate::invalid)?;
        let target = read.target(&params.entity.0)?;
        let resolved = read.schema().resolve_path(read.entity_type()?, &path)?;
        if resolved.field.is_some_and(|field| field.editor.read_only) {
            return Err(crate::invalid("This property is read-only"));
        }
        let entities = read.resolved();
        let entity = entities
            .get(&target.address())
            .ok_or_else(|| crate::invalid("The edited entity no longer exists"))?;
        let mut whole = ara_schema::path::get_effective(&entity.record, &resolved.steps)
            .unwrap_or_else(|| {
                resolved
                    .field
                    .map_or(Value::Null, |field| read.schema().field_default(field))
            });
        let Some(address) = address::resolve(
            read.schema(),
            &resolved.ty,
            &mut whole,
            &params.pointer,
            params.hints.as_deref().unwrap_or_default(),
            params.operation,
        )?
        else {
            return Ok(
                json!({"changed":false,"skipped":true,"entity":params.entity.0,"reason":"This object has no matching collection item or key"}),
            );
        };
        if params.operation == Operation::Move
            && let Some(key) = params.value.get("key").and_then(Value::as_str)
            && address
                .pointer
                .rsplit_once('/')
                .and_then(|(parent, _)| whole.pointer(parent))
                .and_then(Value::as_array)
                .is_none_or(|items| {
                    !items
                        .iter()
                        .any(|item| item.get("id").and_then(Value::as_str) == Some(key))
                })
        {
            return Ok(
                json!({"changed":false,"skipped":true,"entity":params.entity.0,"reason":"This object has no matching destination item"}),
            );
        }
        let changed =
            apply(&mut whole, &address.pointer, &address.ty, &params)? || address.inserted;
        if changed {
            read.check_value(&path, &whole)?;
            cx.set_entity(&target, &path, Some(whole))?;
        }
        Ok(
            json!({"changed":changed,"skipped":false,"entity":params.entity.0,"path":params.path,"pointer":address.pointer}),
        )
    }
}

pub(crate) fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry.command::<Edit>(db);
}

fn apply(
    whole: &mut Value,
    pointer: &str,
    ty: &ValueType,
    params: &Params,
) -> Result<bool, Diagnostic> {
    match params.operation {
        Operation::Set | Operation::Hex | Operation::Append => {
            let at = whole
                .pointer_mut(pointer)
                .ok_or_else(|| crate::invalid("The nested value no longer exists"))?;
            let value = params.value.clone();
            match params.operation {
                Operation::Set => {
                    let changed = *at != value;
                    *at = value;
                    Ok(changed)
                }
                Operation::Hex => {
                    let rgb = value
                        .as_array()
                        .filter(|values| values.len() == 3)
                        .ok_or_else(|| crate::invalid("Expected three linear colour channels"))?;
                    let channels = at
                        .as_array_mut()
                        .filter(|values| values.len() >= 3)
                        .ok_or_else(|| crate::invalid("The property is not a colour"))?;
                    let mut changed = false;
                    for index in 0..3 {
                        changed |= channels[index] != rgb[index];
                        channels[index] = rgb[index].clone();
                    }
                    Ok(changed)
                }
                _ => {
                    if let Some(items) = at.as_array_mut() {
                        let mut value = value;
                        if matches!(ty, ValueType::Keyed(_)) {
                            let key = value.get("id").and_then(Value::as_str).unwrap_or("item");
                            let key = unique(key, |key| {
                                items
                                    .iter()
                                    .any(|item| item.get("id").and_then(Value::as_str) == Some(key))
                            });
                            value
                                .as_object_mut()
                                .ok_or_else(|| {
                                    crate::invalid("A keyed list item must be an object")
                                })?
                                .insert("id".into(), json!(key));
                        }
                        items.push(value);
                    } else if let Some(map) = at.as_object_mut() {
                        let key = value["key"]
                            .as_str()
                            .filter(|key| !key.is_empty())
                            .ok_or_else(|| crate::invalid("A map entry needs a key"))?;
                        let key = unique(key, |key| map.contains_key(key));
                        map.insert(key, value.get("value").cloned().unwrap_or(Value::Null));
                    } else {
                        return Err(crate::invalid("Only a list or map accepts appended items"));
                    }
                    Ok(true)
                }
            }
        }
        Operation::Remove
        | Operation::Move
        | Operation::MoveUp
        | Operation::MoveDown
        | Operation::Rename => {
            let (parent, key) = pointer
                .rsplit_once('/')
                .ok_or_else(|| crate::invalid("Choose an item inside a collection"))?;
            let key = address::decode(key)?;
            let parent = whole
                .pointer_mut(parent)
                .ok_or_else(|| crate::invalid("The collection no longer exists"))?;
            if params.operation == Operation::Rename {
                let map = parent
                    .as_object_mut()
                    .ok_or_else(|| crate::invalid("Only map keys can be renamed"))?;
                let value = params.value.clone();
                let next = value
                    .as_str()
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .ok_or_else(|| crate::invalid("A key cannot be empty"))?;
                if next == key {
                    return Ok(false);
                }
                if map.contains_key(next) {
                    return Err(crate::invalid(format!("`{next}` already exists")));
                }
                let value = map
                    .remove(&key)
                    .ok_or_else(|| crate::invalid("The entry no longer exists"))?;
                map.insert(next.into(), value);
                return Ok(true);
            }
            if let Some(items) = parent.as_array_mut() {
                let index = key
                    .parse::<usize>()
                    .map_err(|_| crate::invalid("Expected an array item"))?;
                if index >= items.len() {
                    return Ok(false);
                }
                if params.operation == Operation::Remove {
                    items.remove(index);
                    return Ok(true);
                }
                let to = match params.operation {
                    Operation::MoveUp => index.saturating_sub(1),
                    Operation::MoveDown => index.saturating_add(1).min(items.len() - 1),
                    _ => {
                        if let Some(key) = params.value.get("key").and_then(Value::as_str) {
                            let target = items
                                .iter()
                                .position(|item| {
                                    item.get("id").and_then(Value::as_str) == Some(key)
                                })
                                .ok_or_else(|| {
                                    crate::invalid("The destination item no longer exists")
                                })?;
                            let insertion = target
                                + usize::from(params.value["after"].as_bool().unwrap_or(false));
                            if index < insertion {
                                insertion - 1
                            } else {
                                insertion
                            }
                        } else {
                            params
                                .index
                                .ok_or_else(|| crate::invalid("Move needs a destination index"))?
                                as usize
                        }
                    }
                }
                .min(items.len() - 1);
                if to == index {
                    return Ok(false);
                }
                let item = items.remove(index);
                items.insert(to, item);
                Ok(true)
            } else if let Some(map) = parent.as_object_mut() {
                if params.operation != Operation::Remove {
                    return Err(crate::invalid("Map entries are ordered by key"));
                }
                Ok(map.remove(&key).is_some())
            } else {
                Err(crate::invalid("The parent value is not a collection"))
            }
        }
    }
}

fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.into();
    }
    (1..)
        .map(|index| format!("{base}{index}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.into())
}
