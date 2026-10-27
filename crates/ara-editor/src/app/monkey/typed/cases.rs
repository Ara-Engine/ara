//! Seven bounded schema-aware probes, rather than a field Cartesian product.
use serde_json::{Value, json};

pub(super) const NAMES: [&str; 7] = [
    "emptyArgs",
    "registeredExample",
    "missingOrDeletedReferences",
    "emptyValues",
    "hugePositiveNumbers",
    "hugeNegativeNumbers",
    "nanAsText",
];

pub(super) fn probe(example: &Value, schema: &Value, case: usize) -> (Value, usize) {
    if case == 0 {
        return (json!({}), 0);
    }
    let mut value = example.clone();
    if case == 1 {
        return (value, 0);
    }
    let changed = mutate(&mut value, schema, schema, case, 0);
    (value, changed)
}

fn mutate(value: &mut Value, schema: &Value, root: &Value, case: usize, depth: usize) -> usize {
    if depth >= 16 {
        return 0;
    }
    if let Some(reference) = schema["$ref"].as_str().and_then(|r| r.strip_prefix('#'))
        && let Some(schema) = root.pointer(reference)
    {
        return mutate(value, schema, root, case, depth + 1);
    }
    if let Some(alternatives) = schema["anyOf"].as_array()
        && let Some(schema) = alternatives.iter().find(|s| s["type"] != "null")
    {
        return mutate(value, schema, root, case, depth + 1);
    }
    let numeric =
        value.is_number() || matches!(schema["type"].as_str(), Some("number" | "integer"));
    if numeric && (4..=6).contains(&case) {
        *value = match case {
            4 => json!(1e308),
            5 => json!(-1e308),
            _ => json!("NaN"),
        };
        return 1;
    }
    if case == 2 {
        let pattern = schema["pattern"].as_str().unwrap_or_default();
        let text = value.as_str().unwrap_or_default();
        if pattern.starts_with("^@") || text.starts_with('@') {
            *value = json!("@levels/deleted.scene#sc_deleted001");
            return 1;
        }
        if pattern.contains("[a-z]{1,3}_") || ara_doc::RecordId::parse(text).is_ok() {
            *value = json!("e_deleted001");
            return 1;
        }
    }
    if case == 3 && matches!(value, Value::String(_) | Value::Array(_)) {
        *value = if value.is_array() {
            json!([])
        } else {
            json!("")
        };
        return 1;
    }
    match value {
        Value::Object(object) => {
            let mut changed = 0;
            // Optional numeric parameters missing from examples are important too.
            if (4..=6).contains(&case)
                && let Some(properties) = schema["properties"].as_object()
            {
                for (name, child) in properties {
                    if !object.contains_key(name) {
                        let mut candidate = Value::Null;
                        let count = mutate(&mut candidate, child, root, case, depth + 1);
                        if count > 0 {
                            object.insert(name.clone(), candidate);
                        }
                    }
                }
            }
            for (name, value) in object.iter_mut() {
                let child = schema["properties"]
                    .get(name)
                    .unwrap_or(&schema["additionalProperties"]);
                changed += mutate(value, child, root, case, depth + 1);
            }
            changed
        }
        Value::Array(array) => array
            .iter_mut()
            .map(|value| mutate(value, &schema["items"], root, case, depth + 1))
            .sum(),
        _ => 0,
    }
}
