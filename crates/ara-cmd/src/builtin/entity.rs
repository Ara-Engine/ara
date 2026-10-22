//! `entity.create`, `entity.delete`, `entity.reparent` and the `entity.inspect` query.

use std::collections::BTreeMap;

use ara_diag::{Diagnostic, codes};
use ara_doc::{Record, RecordId, field, parent_of};
use ara_schema::{EntityRef, FieldPath, Schema, Validator};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Command, EditCtx, Hints, Query, ReadCtx, Target};

/// Parameters of `entity.create`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityCreateParams {
    /// ID of the new entity: a symbol (`$crate`) that later calls in the batch can
    /// use, or a concrete unused record ID. Minted when absent.
    pub id: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// Parent entity; absent creates a root. May be inside a prefab instance.
    pub parent: Option<EntityRef>,
    /// Position among the parent's children; absent appends.
    pub index: Option<u32>,
    /// Components to add, by type name; `{}` uses every default.
    pub components: Option<BTreeMap<String, Value>>,
}

/// The ID of a created entity.
#[derive(Debug, Serialize)]
pub struct Created {
    /// The new record ID.
    pub id: String,
}

/// `entity.create`.
pub struct EntityCreate;

impl Command for EntityCreate {
    type Params = EntityCreateParams;
    type Output = Created;
    const NAME: &'static str = "entity.create";
    const SUMMARY: &'static str =
        "Create an entity, optionally with a parent, a position among its siblings and components.";

    fn examples() -> Vec<Value> {
        vec![
            json!({ "id": "$crate", "name": "Crate", "components": { "Transform": { "position": [2, 0, 3] } } }),
            json!({ "name": "Lid", "parent": "$crate", "index": 0 }),
        ]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: EntityCreateParams) -> Result<Created, Diagnostic> {
        let read = ctx.read();
        let parent = p
            .parent
            .map(|e| read.target(&e.0))
            .transpose()?
            .map(|t| t.address());
        let order = read.order_at(parent.as_deref(), p.index, None)?;
        let mut record = Record::new();
        for (name, value) in p.components.unwrap_or_default() {
            let mut diags = Vec::new();
            Validator::new(read.schema(), &mut diags).component(&name, &value);
            if let Some(d) = diags.into_iter().find(Diagnostic::is_error) {
                let path = d.location.path.clone().unwrap_or_default();
                return Err(d.path(format!(
                    "params.components[{name}]{}",
                    path.strip_prefix(&name).unwrap_or("")
                )));
            }
            record.insert(name, value);
        }
        if let Some(name) = p.name {
            record.insert(field::NAME.into(), name.into());
        }
        if let Some(parent) = parent {
            record.insert(field::PARENT.into(), parent.into());
        }
        record.insert(field::ORDER.into(), order.to_string().into());
        let id = ctx.new_id(p.id.as_deref(), "e")?;
        ctx.create(id.clone(), record)?;
        Ok(Created { id: id.to_string() })
    }
}

/// Parameters of `entity.delete`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityDeleteParams {
    /// Entities to delete, with everything below them. Entities inside a prefab
    /// instance are removed from that instance instead.
    pub entities: Vec<EntityRef>,
}

/// Addresses of deleted entities.
#[derive(Debug, Serialize)]
pub struct Deleted {
    /// Every entity removed, children first.
    pub deleted: Vec<String>,
}

/// `entity.delete`.
pub struct EntityDelete;

impl Command for EntityDelete {
    type Params = EntityDeleteParams;
    type Output = Deleted;
    const NAME: &'static str = "entity.delete";
    const SUMMARY: &'static str =
        "Delete entities and their descendants (prefab entities are removed from their instance).";
    const HINTS: Hints = Hints {
        idempotent: false,
        destructive: true,
    };

    fn examples() -> Vec<Value> {
        vec![json!({ "entities": ["e_8b21"] })]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: EntityDeleteParams) -> Result<Deleted, Diagnostic> {
        let mut deleted = Vec::new();
        for e in &p.entities {
            let target = ctx.read().target(&e.0)?;
            match &target {
                Target::Plain(id)
                | Target::Prefab {
                    instance: id,
                    inner: None,
                } => {
                    for rid in ctx.read().doc().subtree(id) {
                        ctx.delete(&rid)?;
                        deleted.push(rid.to_string());
                    }
                }
                Target::Prefab {
                    instance,
                    inner: Some(inner),
                } => {
                    deleted.extend(remove_from_instance(ctx, instance, inner)?);
                    deleted.push(target.address());
                }
            }
        }
        Ok(Deleted { deleted })
    }
}

/// Removes a prefab entity from an instance: adds it to `removed`, drops overrides
/// aimed at it or below it, and deletes document records parented inside it.
fn remove_from_instance(
    ctx: &mut EditCtx<'_, '_>,
    instance: &RecordId,
    inner: &str,
) -> Result<Vec<String>, Diagnostic> {
    let read = ctx.read();
    let rec = read
        .doc()
        .record(instance.as_str())
        .cloned()
        .unwrap_or_default();
    let within = |key: &str| key == inner || key.starts_with(&format!("{inner}/"));
    let overrides: Vec<String> = rec
        .get(field::OVERRIDES)
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|o| o.keys())
        .filter(|k| k.rsplit_once('/').is_some_and(|(ids, _)| within(ids)))
        .cloned()
        .collect();
    let address = format!("{instance}/{inner}");
    let attached: Vec<RecordId> = read
        .doc()
        .records()
        .iter()
        .filter(|(_, r)| {
            parent_of(r).is_some_and(|p| p == address || p.starts_with(&format!("{address}/")))
        })
        .map(|(id, _)| id.clone())
        .collect();
    let mut removed: Vec<Value> = rec
        .get(field::REMOVED)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    removed.retain(|v| !v.as_str().is_some_and(within));
    removed.push(inner.into());

    let mut deleted = Vec::new();
    for root in attached {
        for rid in ctx.read().doc().subtree(&root) {
            ctx.delete(&rid)?;
            deleted.push(rid.to_string());
        }
    }
    for key in overrides {
        ctx.set(instance, FieldPath::field(field::OVERRIDES).key(key), None)?;
    }
    ctx.set(
        instance,
        FieldPath::field(field::REMOVED),
        Some(Value::Array(removed)),
    )?;
    Ok(deleted)
}

/// Parameters of `entity.reparent`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityReparentParams {
    /// The entity to move. Must be a record of the document (not inside a prefab).
    pub entity: EntityRef,
    /// New parent; absent makes the entity a root.
    pub parent: Option<EntityRef>,
    /// Position among the new siblings; absent appends.
    pub index: Option<u32>,
}

/// The entity's new place.
#[derive(Debug, Serialize)]
pub struct Moved {
    /// New parent address, if any.
    pub parent: Option<String>,
    /// New order key.
    pub order: String,
}

/// `entity.reparent`.
pub struct EntityReparent;

impl Command for EntityReparent {
    type Params = EntityReparentParams;
    type Output = Moved;
    const NAME: &'static str = "entity.reparent";
    const SUMMARY: &'static str =
        "Move an entity under a new parent (or to the root) at a position among its siblings.";
    const HINTS: Hints = Hints {
        idempotent: true,
        destructive: false,
    };

    fn examples() -> Vec<Value> {
        vec![
            json!({ "entity": "e_8b21", "parent": "e_1c0d", "index": 0 }),
            json!({ "entity": "e_8b21" }),
        ]
    }

    fn run(ctx: &mut EditCtx<'_, '_>, p: EntityReparentParams) -> Result<Moved, Diagnostic> {
        let read = ctx.read();
        let id = match read.target(&p.entity.0)? {
            Target::Plain(id)
            | Target::Prefab {
                instance: id,
                inner: None,
            } => id,
            Target::Prefab { .. } => {
                return Err(Diagnostic::error(
                    codes::INVALID_PARAMS,
                    format!(
                        "`{}` is part of a prefab; reparent the instance instead",
                        p.entity.0
                    ),
                ));
            }
        };
        let parent = p
            .parent
            .map(|e| read.target(&e.0))
            .transpose()?
            .map(|t| t.address());
        if let Some(parent) = &parent {
            let mut cur = parent.split('/').next().map(str::to_owned);
            while let Some(c) = cur {
                if c == id.as_str() {
                    return Err(Diagnostic::error(
                        codes::PARENT_CYCLE,
                        format!("`{parent}` is inside `{id}`"),
                    ));
                }
                cur = read
                    .doc()
                    .record(&c)
                    .and_then(parent_of)
                    .and_then(|p| p.split('/').next())
                    .map(str::to_owned);
            }
        }
        let order = read.order_at(parent.as_deref(), p.index, Some(&id))?;
        ctx.move_to(&id, parent.clone(), Some(order.clone()))?;
        Ok(Moved {
            parent,
            order: order.to_string(),
        })
    }
}

/// Parameters of `entity.inspect`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityInspectParams {
    /// The entity to inspect; may be inside a prefab instance.
    pub entity: EntityRef,
}

/// `entity.inspect`: the entity after prefab resolution.
pub struct EntityInspect;

impl Query for EntityInspect {
    type Params = EntityInspectParams;
    type Output = Value;
    const NAME: &'static str = "entity.inspect";
    const SUMMARY: &'static str =
        "Show an entity's name, parent and components with prefab overrides applied.";

    fn examples() -> Vec<Value> {
        vec![json!({ "entity": "e_4f9a/e_m4tz" })]
    }

    fn run(ctx: &ReadCtx<'_>, p: EntityInspectParams) -> Result<Value, Diagnostic> {
        let target = ctx.target(&p.entity.0)?;
        let e = ctx.entity(&target).ok_or_else(|| {
            Diagnostic::error(
                codes::RECORD_NOT_FOUND,
                format!("no entity `{}`", p.entity.0),
            )
        })?;
        Ok(json!({
            "entity": target.address(),
            "parent": e.parent,
            "order": e.order,
            "source": { "doc": e.source.0.to_string(), "record": e.source.1.to_string() },
            "record": e.record,
        }))
    }
}
