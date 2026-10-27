//! `component.add`, `component.remove`, `property.set` and `transform.set`.

use ara_diag::{Diagnostic, codes};
use ara_doc::{field, is_component_key};
use ara_schema::{EntityRef, FieldPath, Schema, TypeDesc, Validator};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{Command, EditCtx, Hints, Target};

/// The entity a command changed.
#[derive(Debug, Serialize)]
pub struct Changed {
    /// Entity address.
    pub entity: String,
}

/// Parameters of `component.add`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentAddParams {
    /// Target entity; may be inside a prefab instance (stored as an override).
    pub entity: EntityRef,
    /// Component type name (`Light`).
    pub component: String,
    /// Initial field values; absent uses every default.
    pub value: Option<Value>,
}

/// `component.add`.
pub struct ComponentAdd;

impl Command for ComponentAdd {
    type Params = ComponentAddParams;
    type Output = Changed;
    const NAME: &'static str = "component.add";
    const SUMMARY: &'static str = "Add a component to an entity, with optional initial values.";

    fn examples() -> Vec<Value> {
        vec![
            json!({ "entity": "e_8b21", "component": "Transform", "value": { "position": [0, 1, 0] } }),
        ]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: ComponentAddParams) -> Result<Changed, Diagnostic> {
        let read = ctx.read();
        let target = read.target(&p.entity.0)?;
        component_type(read.schema(), &p.component)?;
        if read.has_component(&target, &p.component) {
            return Err(Diagnostic::error(
                codes::COMPONENT_EXISTS,
                format!(
                    "`{}` already has `{}`; use `property.set` to change it",
                    target.address(),
                    p.component
                ),
            ));
        }
        let value = p.value.unwrap_or_else(|| Value::Object(Map::new()));
        let mut diags = Vec::new();
        Validator::new(read.schema(), &mut diags).component(&p.component, &value);
        if let Some(d) = diags.into_iter().find(Diagnostic::is_error) {
            return Err(d);
        }
        ctx.set_entity(&target, &FieldPath::field(&p.component), Some(value))?;
        Ok(Changed {
            entity: target.address(),
        })
    }
}

fn component_type<'a>(
    db: &'a ara_schema::SchemaDb,
    name: &str,
) -> Result<&'a TypeDesc, Diagnostic> {
    db.get(name).filter(|t| t.is_component()).ok_or_else(|| {
        let mut msg = format!("unknown component `{name}`");
        if let Some(s) = db.suggest_type(name, TypeDesc::is_component) {
            msg.push_str(&format!("; did you mean `{s}`?"));
        }
        Diagnostic::error(codes::UNKNOWN_TYPE, msg).path("params.component")
    })
}

/// Parameters of `component.remove`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentRemoveParams {
    /// Target entity; may be inside a prefab instance.
    pub entity: EntityRef,
    /// Component type name.
    pub component: String,
}

/// `component.remove`.
pub struct ComponentRemove;

impl Command for ComponentRemove {
    type Params = ComponentRemoveParams;
    type Output = Changed;
    const NAME: &'static str = "component.remove";
    const SUMMARY: &'static str =
        "Remove a component from an entity (for prefab entities, from this instance only).";
    const HINTS: Hints = Hints {
        idempotent: false,
        destructive: true,
    };

    fn examples() -> Vec<Value> {
        vec![json!({ "entity": "e_8b21", "component": "Transform" })]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: ComponentRemoveParams) -> Result<Changed, Diagnostic> {
        let read = ctx.read();
        let target = read.target(&p.entity.0)?;
        if !read.has_component(&target, &p.component) {
            return Err(Diagnostic::error(
                codes::COMPONENT_MISSING,
                format!("`{}` has no `{}`", target.address(), p.component),
            ));
        }
        let Target::Prefab { instance, .. } = &target else {
            ctx.set_entity(&target, &FieldPath::field(&p.component), None)?;
            return Ok(Changed {
                entity: target.address(),
            });
        };
        // Drop every override of the component; if the prefab itself has it, also
        // list it in `removed`.
        let key = target.override_key(&p.component);
        let rec = read
            .doc()
            .record(instance.as_str())
            .cloned()
            .unwrap_or_default();
        let overrides: Vec<String> = rec
            .get(field::OVERRIDES)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|o| o.keys())
            .filter(|k| *k == &key || k.starts_with(&format!("{key}.")))
            .cloned()
            .collect();
        let added_by_override = overrides.contains(&key);
        let mut removed: Vec<Value> = rec
            .get(field::REMOVED)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for k in overrides {
            ctx.set(instance, FieldPath::field(field::OVERRIDES).key(k), None)?;
        }
        if !added_by_override {
            removed.push(key.into());
            ctx.set(
                instance,
                FieldPath::field(field::REMOVED),
                Some(Value::Array(removed)),
            )?;
        }
        Ok(Changed {
            entity: target.address(),
        })
    }
}

/// Parameters of `property.set`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PropertySetParams {
    /// Target entity; inside a prefab instance this writes an override.
    pub entity: EntityRef,
    /// Field path: `name`, or a component field such as `Light.intensity` or
    /// `Transform.position.y`.
    pub path: String,
    /// New value, in the field's schema type and unit.
    pub value: Value,
}

/// `property.set`.
pub struct PropertySet;

impl Command for PropertySet {
    type Params = PropertySetParams;
    type Output = Changed;
    const NAME: &'static str = "property.set";
    const SUMMARY: &'static str = "Set one field of an entity (its name or a component field).";
    const HINTS: Hints = Hints {
        idempotent: true,
        destructive: false,
    };

    fn examples() -> Vec<Value> {
        vec![
            json!({ "entity": "e_8b21", "path": "Transform.position.y", "value": 2 }),
            json!({ "entity": "e_8b21", "path": "name", "value": "Crate" }),
        ]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: PropertySetParams) -> Result<Changed, Diagnostic> {
        let path: FieldPath = p
            .path
            .parse()
            .map_err(|msg| Diagnostic::error(codes::INVALID_PATH, msg).path("params.path"))?;
        let read = ctx.read();
        let target = read.target(&p.entity.0)?;
        // Project metadata and policy use the same reflected record and op path.
        if read.doc().kind == "project"
            && target.address() == "p_settings"
            && path.head() == "settings"
        {
            if path.segments().len() == 1 && !p.value.is_object() {
                return Err(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "project settings must be a JSON object",
                ));
            }
            ctx.set_entity(&target, &path, Some(p.value))?;
            return Ok(Changed {
                entity: target.address(),
            });
        }
        read.check_value(&path, &p.value)?;
        let comp = path.head();
        if is_component_key(comp) && !read.has_component(&target, comp) {
            return Err(Diagnostic::error(
                codes::COMPONENT_MISSING,
                format!(
                    "`{}` has no `{comp}`; add it with `component.add` first",
                    target.address()
                ),
            ));
        }
        ctx.set_entity(&target, &path, Some(p.value))?;
        Ok(Changed {
            entity: target.address(),
        })
    }
}

/// Parameters of `transform.set`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransformSetParams {
    /// Entities to change; each gets a `Transform` if it has none.
    pub entities: Vec<EntityRef>,
    /// Position relative to the parent.
    #[ara(unit = Meters)]
    pub position: Option<[f32; 3]>,
    /// Rotation relative to the parent, as XYZ Euler angles.
    #[ara(unit = Degrees)]
    pub rotation: Option<[f32; 3]>,
    /// Scale relative to the parent.
    pub scale: Option<[f32; 3]>,
}

/// `transform.set`.
pub struct TransformSet;

impl Command for TransformSet {
    type Params = TransformSetParams;
    type Output = Vec<Changed>;
    const NAME: &'static str = "transform.set";
    const SUMMARY: &'static str = "Set the local position, rotation and/or scale of entities.";
    const HINTS: Hints = Hints {
        idempotent: true,
        destructive: false,
    };

    fn examples() -> Vec<Value> {
        vec![json!({ "entities": ["e_8b21"], "position": [2, 0, 3], "rotation": [0, 45, 0] })]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: TransformSetParams) -> Result<Vec<Changed>, Diagnostic> {
        let fields = [
            ("position", p.position),
            ("rotation", p.rotation),
            ("scale", p.scale),
        ];
        let mut changed = Vec::new();
        for e in &p.entities {
            let target = ctx.read().target(&e.0)?;
            if !ctx.read().has_component(&target, "Transform") {
                ctx.set_entity(
                    &target,
                    &FieldPath::field("Transform"),
                    Some(Value::Object(Map::new())),
                )?;
            }
            for (name, v) in fields.iter().filter_map(|(n, v)| v.map(|v| (n, v))) {
                let value = Value::Array(v.iter().map(|&c| json_f32(c)).collect());
                ctx.set_entity(
                    &target,
                    &FieldPath::field("Transform").child(*name),
                    Some(value),
                )?;
            }
            changed.push(Changed {
                entity: target.address(),
            });
        }
        Ok(changed)
    }
}

/// An `f32` as its shortest decimal (`0.1`, not `0.10000000149011612`).
fn json_f32(v: f32) -> Value {
    v.to_string()
        .parse::<f64>()
        .map_or(Value::Null, Value::from)
}
