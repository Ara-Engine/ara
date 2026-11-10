//! Runtime drafts lowered to typed `play.patch` sets, keeping the running values around
//! them: a draft inside a collection writes into the value read fresh, not the stale one.
use std::collections::BTreeMap;

use ara_schema::ValueType;
use serde_json::{Value, json};

use super::{FieldTarget, Inspection};

/// One `{entity, path, value}` set per drafted field, over `fresh` (the entity read again
/// just now). Refuses a positional edit whose collection changed since the inspection.
pub(super) fn sets(inspection: &Inspection, fresh: &Value) -> ara_ui::Result<Vec<Value>> {
    let mut values = BTreeMap::<String, Value>::new();
    for draft in inspection.drafts.values() {
        let (component, field) = draft
            .target
            .path
            .split_once('.')
            .ok_or("Invalid runtime field path")?;
        let live = fresh["components"]
            .get(component)
            .and_then(|v| v.get(field))
            .ok_or("The runtime field was removed; read it again")?;
        let before = inspection
            .components
            .get(component)
            .and_then(|v| v.get(field))
            .ok_or("The read field disappeared")?;
        if positional_parent_changed(before, live, &draft.target.pointer) {
            return Err(
                "This running collection changed while you edited it; read it again first".into(),
            );
        }
        let value = values
            .entry(draft.target.path.clone())
            .or_insert_with(|| live.clone());
        let slot = if draft.target.pointer.is_empty() {
            value
        } else {
            value
                .pointer_mut(&draft.target.pointer)
                .ok_or("The running collection's item was removed")?
        };
        *slot = parse(&draft.target, &draft.input)?;
    }
    Ok(values
        .into_iter()
        .map(|(path, value)| json!({"entity": inspection.entity, "path": path, "value": value}))
        .collect())
}

/// A draft's value: JSON as it is, text parsed by the field's type (whole numbers keep
/// full u64 precision; numbers take their unit; an empty reference is none).
fn parse(target: &FieldTarget, input: &Value) -> ara_ui::Result<Value> {
    let Some(text) = input.as_str() else {
        return Ok(input.clone());
    };
    if let Some((min, max)) = target.ty.int_bounds() {
        let value = text
            .trim()
            .parse::<i128>()
            .map_err(|_| "Enter a whole number")?;
        if value < min || value > max {
            return Err("The whole number is outside this field's range".into());
        }
        return Ok(if value >= 0 {
            json!(value as u64)
        } else {
            json!(value as i64)
        });
    }
    if target.ty.is_number() {
        return Ok(json!(crate::inspector::parse_number(
            text,
            target.unit.as_deref()
        )?));
    }
    if matches!(target.ty, ValueType::Entity | ValueType::Asset(_)) && text.trim().is_empty() {
        return Ok(Value::Null);
    }
    Ok(json!(text))
}

/// Whether an array on the way to `pointer` differs between `before` and `after` (an
/// index would now name another element).
fn positional_parent_changed(before: &Value, after: &Value, pointer: &str) -> bool {
    let mut prefix = String::new();
    for segment in pointer
        .strip_prefix('/')
        .into_iter()
        .flat_map(|p| p.split('/'))
    {
        let (left, right) = match prefix.is_empty() {
            true => (Some(before), Some(after)),
            false => (before.pointer(&prefix), after.pointer(&prefix)),
        };
        if left.is_some_and(Value::is_array) && left != right {
            return true;
        }
        prefix.push('/');
        prefix.push_str(segment);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_edits_refuse_reordered_arrays_but_not_changed_maps() {
        assert!(positional_parent_changed(
            &json!([{"v":1},{"v":2}]),
            &json!([{"v":2},{"v":1}]),
            "/0/v"
        ));
        assert!(!positional_parent_changed(
            &json!({"a":1,"b":2}),
            &json!({"a":1,"b":3}),
            "/a"
        ));
    }

    #[test]
    fn whole_numbers_keep_u64_precision_and_refuse_fractions() {
        let target = FieldTarget {
            path: "Counter.value".into(),
            pointer: String::new(),
            ty: ValueType::U64,
            unit: None,
        };
        assert_eq!(
            parse(&target, &json!("18446744073709551615")).ok(),
            Some(json!(u64::MAX))
        );
        assert!(parse(&target, &json!("1.5")).is_err());
        assert!(parse(&target, &json!("-1")).is_err());
    }
}
