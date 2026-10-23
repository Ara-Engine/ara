//! Inspector model derived entirely from the Schema DB and resolved authoring values.
use ara_doc::prefab::Resolved;
use ara_schema::{FieldDesc, FieldPath, SchemaDb, Shape, ValueType};
use serde::Serialize;
use serde_json::{Value, json};

/// Effective value source, shown next to every inspector field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Provenance {
    /// Omitted authoring data uses its reflected default.
    Default,
    /// Inherited from a prefab document.
    Prefab,
    /// Explicit instance override.
    Override,
    /// Explicit ordinary authoring value.
    Authored,
    /// Applied agent change awaiting review.
    AgentChanged,
}
/// A generated control's semantic kind.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "widget", rename_all = "camelCase")]
pub enum Widget {
    /// Scalar value, optionally with a range/slider.
    Number {
        /// Minimum value.
        min: Option<f64>,
        /// Maximum value.
        max: Option<f64>,
        /// Slider presentation requested by schema.
        slider: bool,
        /// Keyboard/drag increment.
        step: f64,
    },
    /// Vector or colour channels.
    Vector {
        /// Number of channels.
        count: usize,
        /// Colour swatch/picker.
        color: bool,
    },
    /// Boolean checkbox.
    Bool,
    /// Reflected enum choice.
    Enum {
        /// Stable variant names.
        options: Vec<String>,
    },
    /// Asset picker with optional kind restriction.
    Asset {
        /// Required asset kind.
        kind: Option<String>,
    },
    /// Entity address with a scene-picking action.
    Entity,
    /// Text or JSON fallback for compound data.
    Text,
}
/// One inspector field across all selected entities.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// Stable schema field path.
    pub path: String,
    /// User-facing field name.
    pub label: String,
    /// Schema documentation, used as the tooltip/accessibility description.
    pub doc: String,
    /// Common effective value, or null when mixed.
    pub value: Value,
    /// Whether selected entities have differing effective values.
    pub mixed: bool,
    /// Origin of the displayed value.
    pub provenance: Provenance,
    /// Optional previous agent value, for was â†’ now.
    pub was: Option<Value>,
    /// Reflected unit name.
    pub unit: Option<String>,
    /// Generated typed control.
    pub widget: Widget,
    /// Schema marks this property read-only.
    pub read_only: bool,
}
/// Generate common component fields, preserving mixed values and defaults.
pub fn fields(
    db: &SchemaDb,
    document: &ara_doc::Document,
    entities: &Resolved,
    selection: &[String],
    agent_changes: &std::collections::BTreeMap<(String, String), Value>,
) -> Vec<Field> {
    let selected: Vec<_> = selection
        .iter()
        .filter_map(|id| entities.get(id).map(|entity| (id, entity)))
        .collect();
    let Some((_, first)) = selected.first() else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for component in first.record.keys().filter(|k| ara_doc::is_component_key(k)) {
        if !selected
            .iter()
            .all(|(_, e)| e.record.contains_key(component))
        {
            continue;
        }
        let Some(ty) = db.get(component) else {
            continue;
        };
        let Shape::Struct { fields, .. } = &ty.shape else {
            continue;
        };
        for desc in fields.iter().filter(|f| !f.editor.hidden) {
            let path = format!("{component}.{}", desc.name);
            let field_path = match path.parse::<FieldPath>() {
                Ok(p) => p,
                Err(_) => continue,
            };
            let entity_ty = match db.get("Entity") {
                Some(t) => t,
                None => continue,
            };
            let resolved = match db.resolve_path(entity_ty, &field_path) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let values: Vec<_> = selected
                .iter()
                .map(|(_, e)| {
                    ara_schema::path::get_effective(&e.record, &resolved.steps)
                        .unwrap_or_else(|| db.field_default(desc))
                })
                .collect();
            let value = values.first().cloned().unwrap_or(Value::Null);
            let mixed = values.iter().any(|v| *v != value);
            let mut provenance = Provenance::Default;
            let mut was = None;
            for (id, entity) in &selected {
                let root = id.split('/').next().unwrap_or(id);
                let record = document.record(root);
                let override_key = id
                    .strip_prefix(root)
                    .and_then(|s| s.strip_prefix('/'))
                    .map_or_else(|| path.clone(), |inner| format!("{inner}/{path}"));
                let overridden = record
                    .and_then(|r| r.get("overrides"))
                    .and_then(Value::as_object)
                    .is_some_and(|o| {
                        o.keys().any(|k| {
                            k == &override_key || override_key.starts_with(&format!("{k}."))
                        })
                    });
                if overridden {
                    provenance = Provenance::Override;
                } else if entity.source.0 != document.id {
                    provenance = Provenance::Prefab;
                } else if entity
                    .record
                    .get(component)
                    .and_then(|v| v.get(&desc.name))
                    .is_some()
                {
                    provenance = Provenance::Authored;
                }
                for ((changed_entity, changed), previous) in agent_changes {
                    if changed_entity != *id {
                        continue;
                    }
                    if changed == &path {
                        provenance = Provenance::AgentChanged;
                        was = Some(if previous.is_null() {
                            db.field_default(desc)
                        } else {
                            previous.clone()
                        });
                    } else if path.starts_with(&format!("{changed}.")) {
                        provenance = Provenance::AgentChanged;
                        was = Some(
                            previous
                                .get(&desc.name)
                                .cloned()
                                .unwrap_or_else(|| db.field_default(desc)),
                        );
                    } else if let Some(axis) = changed.strip_prefix(&format!("{path}."))
                        && let Some(index) = desc.ty.component_index(axis)
                    {
                        let mut original = value.clone();
                        if let Some(values) = original.as_array_mut()
                            && index < values.len()
                        {
                            values[index] = if previous.is_null() {
                                db.field_default(desc)[index].clone()
                            } else {
                                previous.clone()
                            };
                            was = Some(original);
                            provenance = Provenance::AgentChanged;
                        }
                    }
                }
            }
            result.push(Field {
                path,
                label: desc.name.clone(),
                doc: desc.doc.clone(),
                value: if mixed { Value::Null } else { value },
                mixed,
                provenance,
                was,
                unit: desc.unit.map(|u| u.symbol().to_owned()),
                widget: widget(db, desc),
                read_only: desc.editor.read_only,
            });
        }
    }
    result
}
/// Map reflected field metadata to the shared native control model.
pub(crate) fn widget(db: &SchemaDb, desc: &FieldDesc) -> Widget {
    let ty = if let ValueType::Option(inner) = &desc.ty {
        inner.as_ref()
    } else {
        &desc.ty
    };
    if ty.is_number() {
        return Widget::Number {
            min: desc.range.and_then(|r| r.min),
            max: desc.range.and_then(|r| r.max),
            slider: desc.editor.slider,
            step: desc.editor.step.unwrap_or(0.1),
        };
    }
    if let Some(count) = ty.vector_len() {
        return Widget::Vector {
            count,
            color: *ty == ValueType::Color,
        };
    }
    match ty {
        ValueType::Bool => Widget::Bool,
        ValueType::Asset(kind) => Widget::Asset { kind: kind.clone() },
        ValueType::Entity => Widget::Entity,
        ValueType::Named(name) => match db.get(name).map(|t| &t.shape) {
            Some(Shape::Enum { variants }) => Widget::Enum {
                options: variants.iter().map(|v| v.name.clone()).collect(),
            },
            _ => Widget::Text,
        },
        _ => Widget::Text,
    }
}
/// Parse a scalar with a physical suffix, converting to the schema's authoring unit.
pub fn parse_number(input: &str, unit: Option<&str>) -> Result<f64, String> {
    let input = input.trim();
    let end = input
        .char_indices()
        .find(|(_, c)| c.is_alphabetic() && *c != 'e' && *c != 'E')
        .map_or(input.len(), |(i, _)| i);
    let (number, suffix) = input.split_at(end);
    let number: f64 = number
        .trim()
        .parse()
        .map_err(|_| "Enter a finite number, optionally followed by a unit".to_owned())?;
    if !number.is_finite() {
        return Err("Value must be finite".into());
    }
    let unit = unit.unwrap_or("");
    let factor = match (suffix.trim(), unit) {
        ("", _) => 1.,
        ("m", "m" | "meters") => 1.,
        ("cm", "m" | "meters") => 0.01,
        ("mm", "m" | "meters") => 0.001,
        ("km", "m" | "meters") => 1000.,
        ("deg", "deg" | "degrees") => 1.,
        ("rad", "deg" | "degrees") => 180. / std::f64::consts::PI,
        ("s", "s" | "seconds") => 1.,
        ("ms", "s" | "seconds") => 0.001,
        (s, u) if s == u => 1.,
        _ => return Err(format!("Unit `{suffix}` is incompatible with `{unit}`")),
    };
    Ok(number * factor)
}
/// Build one property.set per selected entity; a caller commits the list atomically.
pub fn edit_commands(selection: &[String], path: &str, value: &Value) -> Vec<Value> {
    selection.iter().map(|entity|json!({"cmd":"property.set","args":{"entity":entity,"path":path,"value":value}})).collect()
}
