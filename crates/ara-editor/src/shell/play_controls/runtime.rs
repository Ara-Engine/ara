//! Preserve unrelated runtime fields while lowering staged edits to typed sets.
use super::*;
pub(super) fn sets(runtime: &Runtime, fresh: &Value) -> ara_ui::Result<Vec<Value>> {
    let mut values = BTreeMap::<String, Value>::new();
    for draft in runtime.drafts.values() {
        let (component, field) = draft
            .target
            .path
            .split_once('.')
            .ok_or("Invalid runtime field path")?;
        let live = fresh["components"]
            .get(component)
            .and_then(|value| value.get(field))
            .ok_or("The runtime field was removed; inspect again")?;
        let before = runtime
            .components
            .get(component)
            .and_then(|value| value.get(field))
            .ok_or("The inspected field disappeared")?;
        if positional_parent_changed(before, live, &draft.target.pointer) {
            return Err("This runtime collection changed while editing. Inspect again before applying the positional edit".into());
        }
        let value = values
            .entry(draft.target.path.clone())
            .or_insert_with(|| live.clone());
        let slot = if draft.target.pointer.is_empty() {
            value
        } else {
            value
                .pointer_mut(&draft.target.pointer)
                .ok_or("The runtime collection item was removed")?
        };
        *slot = parse(&draft.target, &draft.input)?;
    }
    Ok(values
        .into_iter()
        .map(|(path, value)| json!({"entity":runtime.entity,"path":path,"value":value}))
        .collect())
}
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
fn positional_parent_changed(before: &Value, after: &Value, pointer: &str) -> bool {
    let mut prefix = String::new();
    for segment in pointer
        .strip_prefix('/')
        .into_iter()
        .flat_map(|pointer| pointer.split('/'))
    {
        let left = if prefix.is_empty() {
            Some(before)
        } else {
            before.pointer(&prefix)
        };
        let right = if prefix.is_empty() {
            Some(after)
        } else {
            after.pointer(&prefix)
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
    fn positional_edits_reject_reordered_arrays_but_preserve_other_map_entries() {
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
    fn runtime_integer_text_keeps_full_u64_precision_and_rejects_fractional_input() {
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
