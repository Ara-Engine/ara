//! `prefab.instantiate`, `prefab.override.set` and `prefab.override.revert`.

use ara_diag::{Diagnostic, codes};
use ara_doc::refs::resolve;
use ara_doc::{Record, field};
use ara_schema::{AssetRef, EntityRef, FieldPath, Schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::component::Changed;
use super::entity::Created;
use crate::{Command, EditCtx, Hints, Target};

/// Parameters of `prefab.instantiate`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrefabInstantiateParams {
    /// The prefab, as `@path#id`, `@path` or `@#id`.
    #[ara(asset_kind = "prefab")]
    pub prefab: AssetRef,
    /// ID of the instance: a symbol (`$door`) or a concrete unused record ID. Minted
    /// when absent.
    pub id: Option<String>,
    /// Display name; absent keeps the prefab root's name.
    pub name: Option<String>,
    /// Parent entity; absent creates a root.
    pub parent: Option<EntityRef>,
    /// Position among the parent's children; absent appends.
    pub index: Option<u32>,
    /// Local position of the instance.
    #[ara(unit = Meters)]
    pub position: Option<[f32; 3]>,
}

/// `prefab.instantiate`.
pub struct PrefabInstantiate;

impl Command for PrefabInstantiate {
    type Params = PrefabInstantiateParams;
    type Output = Created;
    const NAME: &'static str = "prefab.instantiate";
    const SUMMARY: &'static str =
        "Place an instance of a prefab; its content stays linked to the prefab.";

    fn examples() -> Vec<Value> {
        vec![json!({ "prefab": "@prefabs/crate.prefab", "id": "$crate", "position": [0, 0, 4] })]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: PrefabInstantiateParams) -> Result<Created, Diagnostic> {
        let read = ctx.read();
        let (reference, _) = resolve(&p.prefab.0, Some("prefab"), read.workspace().index())
            .map_err(|d| d.path("params.prefab"))?;
        let loaded = ara_schema::grammar::parse_ref(&reference)
            .ok()
            .and_then(|r| r.id)
            .is_some_and(|id| read.workspace().doc(id).is_some());
        if !loaded {
            return Err(Diagnostic::error(
                codes::PREFAB_NOT_FOUND,
                format!("prefab `{reference}` is not loaded"),
            )
            .path("params.prefab"));
        }
        let parent = p
            .parent
            .map(|e| read.target(&e.0))
            .transpose()?
            .map(|t| t.address());
        let order = read.order_at(parent.as_deref(), p.index, None)?;
        let mut record = Record::new();
        if let Some(name) = p.name {
            record.insert(field::NAME.into(), name.into());
        }
        if let Some(parent) = parent {
            record.insert(field::PARENT.into(), parent.into());
        }
        record.insert(field::ORDER.into(), order.to_string().into());
        record.insert(field::PREFAB.into(), reference.into());
        if let Some(pos) = p.position {
            let mut overrides = Map::new();
            overrides.insert("Transform.position".into(), json!(pos.map(f64::from)));
            record.insert(field::OVERRIDES.into(), Value::Object(overrides));
        }
        let id = ctx.new_id(p.id.as_deref(), "e")?;
        ctx.create(id.clone(), record)?;
        Ok(Created { id: id.to_string() })
    }
}

fn instance_target(ctx: &EditCtx<'_, '_>, entity: &str) -> Result<Target, Diagnostic> {
    match ctx.read().target(entity)? {
        t @ Target::Prefab { .. } => Ok(t),
        Target::Plain(id) => Err(Diagnostic::error(
            codes::NOT_AN_INSTANCE,
            format!("`{id}` is not a prefab instance or inside one; use `property.set`"),
        )),
    }
}

/// Parameters of `prefab.override.set`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverrideSetParams {
    /// A prefab instance, or an entity inside one (`e_4f9a/e_m4tz`).
    pub entity: EntityRef,
    /// Field path: `name`, a component (`Light`) or a component field (`Light.color`).
    pub path: String,
    /// The overriding value.
    pub value: Value,
}

/// `prefab.override.set`.
pub struct OverrideSet;

impl Command for OverrideSet {
    type Params = OverrideSetParams;
    type Output = Changed;
    const NAME: &'static str = "prefab.override.set";
    const SUMMARY: &'static str =
        "Override a value of a prefab instance (or of an entity inside it).";
    const HINTS: Hints = Hints {
        idempotent: true,
        destructive: false,
    };

    fn examples() -> Vec<Value> {
        vec![
            json!({ "entity": "e_4f9a/e_m4tz", "path": "MeshRenderer.tint", "value": [1, 0, 0, 1] }),
        ]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: OverrideSetParams) -> Result<Changed, Diagnostic> {
        let path: FieldPath = p
            .path
            .parse()
            .map_err(|msg| Diagnostic::error(codes::INVALID_PATH, msg).path("params.path"))?;
        let target = instance_target(ctx, &p.entity.0)?;
        ctx.read().check_value(&path, &p.value)?;
        ctx.set_entity(&target, &path, Some(p.value))?;
        Ok(Changed {
            entity: target.address(),
        })
    }
}

/// Parameters of `prefab.override.revert`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverrideRevertParams {
    /// A prefab instance, or an entity inside one.
    pub entity: EntityRef,
    /// The overridden path to revert, with everything under it; absent reverts every
    /// override and removal on this entity.
    pub path: Option<String>,
}

/// Override keys that were reverted.
#[derive(Debug, Serialize)]
pub struct Reverted {
    /// Reverted override keys and `removed` entries.
    pub reverted: Vec<String>,
}

/// `prefab.override.revert`.
pub struct OverrideRevert;

impl Command for OverrideRevert {
    type Params = OverrideRevertParams;
    type Output = Reverted;
    const NAME: &'static str = "prefab.override.revert";
    const SUMMARY: &'static str =
        "Revert overrides of a prefab instance back to the prefab's values.";
    const HINTS: Hints = Hints {
        idempotent: true,
        destructive: false,
    };

    fn examples() -> Vec<Value> {
        vec![
            json!({ "entity": "e_4f9a/e_m4tz", "path": "MeshRenderer.tint" }),
            json!({ "entity": "e_4f9a" }),
        ]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: OverrideRevertParams) -> Result<Reverted, Diagnostic> {
        let target = instance_target(ctx, &p.entity.0)?;
        let Target::Prefab { instance, inner } = &target else {
            return Ok(Reverted {
                reverted: Vec::new(),
            });
        };
        let rec = ctx
            .read()
            .doc()
            .record(instance.as_str())
            .cloned()
            .unwrap_or_default();
        let base = p.path.as_deref().map(|path| target.override_key(path));
        let matches = |key: &str| match &base {
            Some(b) => {
                key == b || key.starts_with(&format!("{b}.")) || key.starts_with(&format!("{b}["))
            }
            None => {
                let ids = key.rsplit_once('/').map(|(ids, _)| ids);
                ids == inner.as_deref()
            }
        };
        let keys: Vec<String> = rec
            .get(field::OVERRIDES)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|o| o.keys())
            .filter(|k| matches(k))
            .cloned()
            .collect();
        let mut reverted = keys.clone();
        for key in keys {
            ctx.set(instance, FieldPath::field(field::OVERRIDES).key(key), None)?;
        }
        if base.is_none() {
            let removed: Vec<Value> = rec
                .get(field::REMOVED)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let (gone, kept): (Vec<Value>, Vec<Value>) = removed
                .into_iter()
                .partition(|v| v.as_str().is_some_and(matches));
            if !gone.is_empty() {
                reverted.extend(gone.iter().filter_map(Value::as_str).map(str::to_owned));
                let value = (!kept.is_empty()).then_some(Value::Array(kept));
                ctx.set(instance, FieldPath::field(field::REMOVED), value)?;
            }
        }
        Ok(Reverted { reverted })
    }
}
