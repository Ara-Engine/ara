use crate::{Result, SaveGame, codes, error};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One structured persistent field change; absent differs from an explicit null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldDiff {
    /// RFC 6901 JSON Pointer into persistent state (escaped entity IDs/map keys).
    pub path: String,
    /// Prior value; None means the field/entity/component did not exist.
    pub before: Option<Value>,
    /// New value; None means it was removed.
    pub after: Option<Value>,
}
/// Diffs migrated persistent fields and the tick/RNG cursor, excluding slot metadata.
/// Objects recurse by sorted key; lists recurse by index, with explicit additions/removals.
pub fn diff(before: &SaveGame, after: &SaveGame) -> Result<Vec<FieldDiff>> {
    let before =
        serde_json::to_value(&before.state).map_err(|e| error(codes::SCHEMA, e.to_string()))?;
    let after =
        serde_json::to_value(&after.state).map_err(|e| error(codes::SCHEMA, e.to_string()))?;
    let mut out = Vec::new();
    walk("", Some(&before), Some(&after), &mut out);
    Ok(out)
}
fn walk(path: &str, before: Option<&Value>, after: Option<&Value>, out: &mut Vec<FieldDiff>) {
    if before == after {
        return;
    }
    match (before, after) {
        (Some(Value::Object(a)), Some(Value::Object(b))) => {
            let keys: std::collections::BTreeSet<_> = a.keys().chain(b.keys()).collect();
            for key in keys {
                walk(
                    &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                    a.get(key),
                    b.get(key),
                    out,
                );
            }
        }
        (Some(Value::Array(a)), Some(Value::Array(b))) => {
            for index in 0..a.len().max(b.len()) {
                walk(&format!("{path}/{index}"), a.get(index), b.get(index), out);
            }
        }
        _ => out.push(FieldDiff {
            path: path.into(),
            before: before.cloned(),
            after: after.cloned(),
        }),
    }
}
