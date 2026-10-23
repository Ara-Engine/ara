//! Live editing of a running world. Authoring edits become baseline-checked ops that
//! apply at the next tick boundary (and enter replays). "Keep changes" diffs the world
//! against the start documents and returns field-level changes for the edit-world
//! owner to commit as one typed-command transaction.
use crate::{Result, Runtime, RuntimeError};
use ara_doc::{Edit, Op, RecordId, is_component_key};
use ara_schema::ValueType;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One authored value that differs between the play world and the start documents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// Entity address in the scene.
    pub entity: String,
    /// `Component` or `Component.field`; empty for a deleted entity.
    pub path: String,
    /// The play value; absent when the component or entity no longer exists.
    pub value: Option<Value>,
    /// The start value (canonical; absent means default/not stored), for conflict checks.
    pub prev: Option<Value>,
    /// Written by a runtime system every tick (physics, navigation, input), so usually
    /// not a deliberate edit; editors untick these by default.
    pub driven: bool,
}

/// Components every simulated entity rewrites each tick.
const MIRRORS: [&str; 5] = [
    "PhysicsPose",
    "Velocity",
    "CharacterMotion",
    "CharacterInput",
    "VehicleTelemetry",
];

impl Runtime {
    /// Converts edits against the scene's mirrored documents into ops (with `prev`)
    /// and queues them for the next tick. Several patches before one tick chain.
    /// On error nothing is queued.
    pub fn patch_edits(&mut self, edits: Vec<Edit>) -> Result<Vec<Op>> {
        let mirror = self
            .pending
            .get_or_insert_with(|| self.play.session().projection().workspace().clone());
        let mut ops = Vec::new();
        for edit in edits {
            let doc_id = edit.doc().clone();
            let applied = mirror
                .doc_mut(doc_id.as_str())
                .ok_or_else(|| RuntimeError(format!("document {doc_id} is not part of this world")))
                .and_then(|doc| {
                    doc.apply(edit, &self.schema)
                        .map_err(|d| RuntimeError(d.message))
                });
            match applied {
                Ok(op) => ops.push(op),
                Err(error) => {
                    for op in ops.iter().rev() {
                        if let Some(doc) = mirror.doc_mut(op.doc().as_str()) {
                            doc.revert(op, &self.schema);
                        }
                    }
                    return Err(error);
                }
            }
        }
        self.patch(ops.clone())?;
        Ok(ops)
    }

    /// Field-level differences between the play world and the start documents.
    pub fn changes(&self) -> Result<Vec<Change>> {
        let session = self.play.session();
        let scene = session.projection().scene().clone();
        let doc = self
            .baseline
            .doc(scene.as_str())
            .ok_or_else(|| RuntimeError("start scene document is missing".into()))?;
        let mut out = Vec::new();
        for (id, record) in doc.records() {
            let address = id.as_str();
            if session.entity(address).is_none() {
                out.push(Change {
                    entity: address.into(),
                    path: String::new(),
                    value: None,
                    prev: None,
                    driven: false,
                });
                continue;
            }
            let simulated = [
                "RigidBody",
                "CharacterController",
                "NavAgent",
                "FollowCamera",
            ]
            .iter()
            .any(|c| record.contains_key(*c))
                && !record
                    .get("RigidBody")
                    .is_some_and(|b| b.get("motion").is_none_or(|m| m == "static"));
            for component in self.schema.components() {
                let name = component.name.as_str();
                if !is_component_key(name) {
                    continue;
                }
                let ty = ValueType::Named(name.into());
                let world = session
                    .component(address, name)?
                    .map(|v| self.schema.canonical_value(&ty, &v));
                let stored = record
                    .get(name)
                    .map(|v| self.schema.canonical_value(&ty, v));
                let driven_component =
                    MIRRORS.contains(&name) || (simulated && name == "Transform");
                match (&world, &stored) {
                    (None, None) => {}
                    (Some(w), Some(s)) if same(w, s) => {}
                    (Some(Value::Object(w)), Some(Value::Object(s))) => {
                        for field in component.fields() {
                            // Omitted fields are defaults; compare materialized values.
                            let default = self.schema.field_default(field);
                            let wf = w.get(&field.name).unwrap_or(&default);
                            let sf = s.get(&field.name).unwrap_or(&default);
                            if !same(wf, sf) {
                                out.push(Change {
                                    entity: address.into(),
                                    path: format!("{name}.{}", field.name),
                                    value: Some(tidy(wf.clone())),
                                    prev: s.get(&field.name).cloned(),
                                    driven: driven_component || driven_field(name, &field.name),
                                });
                            }
                        }
                    }
                    _ => out.push(Change {
                        entity: address.into(),
                        path: name.into(),
                        value: world.clone().map(tidy),
                        prev: stored.clone(),
                        driven: driven_component,
                    }),
                }
            }
        }
        Ok(out)
    }
}

/// World values are f32s widened to JSON (`0.2` reads back as `0.20000000298…`).
/// Numbers that are exact f32 values are rewritten to their shortest f32 decimal so
/// kept edits and reports carry the authored precision; integers and genuine f64s
/// are unchanged.
pub fn tidy(value: Value) -> Value {
    match value {
        Value::Number(n) if !n.is_i64() && !n.is_u64() => {
            let Some(x) = n.as_f64() else {
                return Value::Number(n);
            };
            let f = x as f32;
            if f64::from(f) != x || !f.is_finite() {
                return Value::Number(n);
            }
            f.to_string()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map_or(Value::Number(n), Value::Number)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(tidy).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, tidy(v))).collect()),
        other => other,
    }
}

/// Value equality at the precision the world stores: floats compare as f32 (world
/// values are f32 widened to JSON), integers exactly.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(x), Some(y)) => x == y,
            _ => x.as_f64().map(|v| v as f32) == y.as_f64().map(|v| v as f32),
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// Fields that runtime systems own even on otherwise authored components.
fn driven_field(component: &str, field: &str) -> bool {
    matches!(
        (component, field),
        ("PlayerControl", "yaw")
            | ("FollowCamera", "yaw" | "pitch")
            | ("Wanderer", "resumeTick" | "goals")
    )
}

/// Builds a `Set` edit for `entity.path` in `doc`.
pub fn set_edit(
    doc: &ara_doc::AssetId,
    entity: &str,
    path: &str,
    value: Option<Value>,
) -> Result<Edit> {
    Ok(Edit::Set {
        doc: doc.clone(),
        id: RecordId::parse(entity).map_err(RuntimeError)?,
        path: path.parse().map_err(RuntimeError)?,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::tidy;
    use serde_json::json;

    #[test]
    fn f32_noise_is_tidied_but_real_values_are_kept() {
        let widened = json!([f64::from(0.2f32), 1.0, 3, f64::from(-0.75f32)]);
        assert_eq!(tidy(widened), json!([0.2, 1.0, 3, -0.75]));
        assert_eq!(tidy(json!(0.1)), json!(0.1), "a genuine f64 is untouched");
    }
}
