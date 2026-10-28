//! Schema-guided JSON-pointer validation and stable keyed-item interpretation.
use super::*;
use ara_schema::Shape;

pub(super) struct Address {
    pub pointer: String,
    pub ty: ValueType,
    pub inserted: bool,
}

pub(crate) fn decode(token: &str) -> Result<String, Diagnostic> {
    let mut decoded = String::new();
    let mut chars = token.chars();
    while let Some(character) = chars.next() {
        decoded.push(if character == '~' {
            match chars.next() {
                Some('0') => '~',
                Some('1') => '/',
                _ => return Err(crate::invalid("A JSON-pointer escape must be ~0 or ~1")),
            }
        } else {
            character
        });
    }
    Ok(decoded)
}

fn tokens(pointer: &str) -> Result<Vec<String>, Diagnostic> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    if !pointer.starts_with('/') {
        return Err(crate::invalid("A nested pointer must start with /"));
    }
    pointer.split('/').skip(1).map(decode).collect()
}

fn token(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn unoptional(ty: &ValueType) -> &ValueType {
    match ty {
        ValueType::Option(inner) => unoptional(inner),
        _ => ty,
    }
}

pub(super) fn resolve(
    db: &SchemaDb,
    ty: &ValueType,
    whole: &mut Value,
    pointer: &str,
    hints: &[AddressHint],
    operation: Operation,
) -> Result<Option<Address>, Diagnostic> {
    let tokens = tokens(pointer)?;
    let mut seen = std::collections::BTreeSet::new();
    if hints.iter().any(|hint| {
        hint.depth as usize >= tokens.len()
            || !seen.insert(hint.depth)
            || hint.key.as_deref() == Some("")
    }) {
        return Err(crate::invalid(
            "Nested address hints must name distinct valid token depths",
        ));
    }
    let mut actual = Vec::new();
    let mut ty = unoptional(ty).clone();
    let mut at = whole;
    let mut inserted = false;
    for (depth, token) in tokens.iter().enumerate() {
        ty = unoptional(&ty).clone();
        let hint = hints.iter().find(|hint| hint.depth as usize == depth);
        let key = match &ty {
            ValueType::List(element) | ValueType::Keyed(element) => {
                let Some(items) = at.as_array_mut() else {
                    return Ok(None);
                };
                let mut index = token
                    .parse::<usize>()
                    .map_err(|_| crate::invalid("A list pointer token must be an item index"))?;
                if let Some(hint) = hint {
                    if let Some(key) = &hint.key {
                        if !matches!(ty, ValueType::Keyed(_)) {
                            return Err(crate::invalid("Only keyed lists accept item-ID hints"));
                        }
                        let Some(found) = items
                            .iter()
                            .position(|item| item.get("id").and_then(Value::as_str) == Some(key))
                        else {
                            return Ok(None);
                        };
                        index = found;
                    } else if hint.append {
                        if operation != Operation::Set {
                            return Err(crate::invalid(
                                "Only a value edit can use an append address",
                            ));
                        }
                        index = items.len();
                    }
                }
                if index == items.len() && hint.is_some_and(|hint| hint.append) {
                    let mut item = element_default(db, element);
                    if matches!(ty, ValueType::Keyed(_)) {
                        let key = super::unique("item", |key| {
                            items
                                .iter()
                                .any(|item| item.get("id").and_then(Value::as_str) == Some(key))
                        });
                        item.as_object_mut()
                            .ok_or_else(|| crate::invalid("A keyed item must be an object"))?
                            .insert("id".into(), json!(key));
                    }
                    items.push(item);
                    inserted = true;
                }
                if index >= items.len() {
                    return Ok(None);
                }
                ty = (**element).clone();
                index.to_string()
            }
            ValueType::Map(element) => {
                if hint.is_some() {
                    return Err(crate::invalid("Map keys do not use array address hints"));
                }
                let Some(map) = at.as_object() else {
                    return Ok(None);
                };
                if !map.contains_key(token) {
                    return Ok(None);
                }
                ty = (**element).clone();
                token.clone()
            }
            ValueType::Named(name) => {
                let Some(Shape::Struct { fields, .. }) = db.get(name).map(|desc| &desc.shape)
                else {
                    return Err(crate::invalid("The nested pointer enters a scalar value"));
                };
                let field = fields
                    .iter()
                    .find(|field| field.name == *token)
                    .ok_or_else(|| crate::invalid(format!("No nested field `{token}`")))?;
                if field.editor.read_only {
                    return Err(crate::invalid("This nested property is read-only"));
                }
                let Some(map) = at.as_object_mut() else {
                    return Ok(None);
                };
                if !map.contains_key(token) {
                    map.insert(token.clone(), db.field_default(field));
                }
                ty = field.ty.clone();
                token.clone()
            }
            _ if ty.vector_len().is_some() => {
                let index = token
                    .parse::<usize>()
                    .map_err(|_| crate::invalid("A vector pointer token must name a channel"))?;
                if at.as_array().is_none_or(|values| index >= values.len()) {
                    return Ok(None);
                }
                ty = ValueType::Json;
                index.to_string()
            }
            ValueType::Json => {
                if at.get(token).is_some() {
                    token.clone()
                } else if let Some(items) = at.as_array() {
                    let index = token
                        .parse::<usize>()
                        .map_err(|_| crate::invalid("Expected an array item index"))?;
                    if index >= items.len() {
                        return Ok(None);
                    }
                    index.to_string()
                } else {
                    return Ok(None);
                }
            }
            _ => return Err(crate::invalid("The nested pointer enters a scalar value")),
        };
        actual.push(key.clone());
        at = match at {
            Value::Array(items) => {
                &mut items[key
                    .parse::<usize>()
                    .map_err(|_| crate::invalid("Invalid nested item index"))?]
            }
            Value::Object(map) => map
                .get_mut(&key)
                .ok_or_else(|| crate::invalid("The nested field disappeared"))?,
            _ => return Ok(None),
        };
    }
    Ok(Some(Address {
        pointer: actual
            .iter()
            .map(|key| format!("/{}", token(key)))
            .collect(),
        ty: unoptional(&ty).clone(),
        inserted,
    }))
}

/// Capture semantic array identities from the visible reference object.
pub(crate) fn hints(
    db: &SchemaDb,
    ty: &ValueType,
    value: &Value,
    pointer: &str,
) -> Result<Vec<AddressHint>, Diagnostic> {
    let tokens = tokens(pointer)?;
    let mut ty = unoptional(ty);
    let mut at = value;
    let mut hints = Vec::new();
    for (depth, token) in tokens.iter().enumerate() {
        ty = unoptional(ty);
        match ty {
            ValueType::List(element) | ValueType::Keyed(element) => {
                let Some(items) = at.as_array() else {
                    break;
                };
                let index = token
                    .parse::<usize>()
                    .map_err(|_| crate::invalid("Expected a list item index"))?;
                let key = if matches!(ty, ValueType::Keyed(_)) {
                    items
                        .get(index)
                        .and_then(|item| item.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                } else {
                    None
                };
                if key.is_some() || index == items.len() {
                    hints.push(AddressHint {
                        depth: depth as u32,
                        key,
                        append: index == items.len(),
                    });
                }
                let Some(item) = items.get(index) else {
                    break;
                };
                at = item;
                ty = element;
            }
            ValueType::Map(element) => {
                let Some(item) = at.get(token) else {
                    break;
                };
                at = item;
                ty = element;
            }
            ValueType::Named(name) => {
                let Some(Shape::Struct { fields, .. }) = db.get(name).map(|desc| &desc.shape)
                else {
                    break;
                };
                let Some(field) = fields.iter().find(|field| field.name == *token) else {
                    break;
                };
                let Some(item) = at.get(token) else {
                    break;
                };
                at = item;
                ty = &field.ty;
            }
            _ => {
                let child = if let Some(items) = at.as_array() {
                    token
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| items.get(index))
                } else {
                    at.get(token)
                };
                let Some(child) = child else {
                    break;
                };
                at = child;
            }
        }
    }
    Ok(hints)
}

fn element_default(db: &SchemaDb, ty: &ValueType) -> Value {
    if let ValueType::Named(name) = unoptional(ty)
        && let Some(Shape::Struct { fields, .. }) = db.get(name).map(|desc| &desc.shape)
    {
        return Value::Object(
            fields
                .iter()
                .map(|field| (field.name.clone(), db.field_default(field)))
                .collect(),
        );
    }
    db.zero(ty)
}
