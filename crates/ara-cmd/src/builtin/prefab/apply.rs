//! Apply source edits and clear local overrides in one cross-document transaction.
use super::*;
use crate::builtin::{
    ComponentAdd, ComponentAddParams, ComponentRemove, ComponentRemoveParams, EntityDelete,
    EntityDeleteParams, PropertySet, PropertySetParams,
};
use ara_doc::{AssetId, field, is_component_key};

/// Parameters for atomic prefab application.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrefabApplyParams {
    /// Instance or nested entity whose overrides are applied.
    pub entity: EntityRef,
    /// A single reflected field or component; absent applies its whole subtree.
    pub path: Option<String>,
    /// A source returned by prefab_sources; absent keeps the nearest prefab/variant boundary.
    #[ara(asset_kind = "prefab")]
    pub destination: Option<AssetRef>,
}

/// Atomically update the chosen source and make this instance follow it again.
pub struct PrefabApply;
impl Command for PrefabApply {
    type Params = PrefabApplyParams;
    type Output = Value;
    const NAME: &'static str = "prefab.apply";
    const SUMMARY: &'static str =
        "Apply overrides and removals to a chosen prefab source in one undoable transaction.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_4f9a"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, params: PrefabApplyParams) -> Result<Value, Diagnostic> {
        let target = instance_target(ctx, &params.entity.0)?;
        let entity = target.address();
        let anchor = ctx.read().doc().id.clone();
        let sources =
            super::sources::prefab_sources(ctx.read().workspace(), anchor.as_str(), &entity)?;
        let destination = if let Some(reference) = params.destination {
            let (reference, _) =
                resolve(&reference.0, Some("prefab"), ctx.read().workspace().index())?;
            let id = ara_schema::grammar::parse_ref(&reference)
                .map_err(|error| Diagnostic::error(codes::PREFAB_NOT_FOUND, error))?
                .id
                .ok_or_else(|| invalid("The chosen source has no identity"))?;
            sources
                .iter()
                .find(|source| source.doc.as_str() == id)
                .ok_or_else(|| invalid("Choose a prefab in this entity's source chain"))?
                .clone()
        } else {
            sources
                .first()
                .cloned()
                .ok_or_else(|| invalid("This entity has no prefab source"))?
        };
        let instance = target.record().clone();
        let record = ctx
            .read()
            .doc()
            .record(instance.as_str())
            .cloned()
            .ok_or_else(|| invalid("The instance no longer exists"))?;
        let selected = |address: &str, path: &str| {
            if let Some(wanted) = &params.path {
                address == entity
                    && (path == wanted
                        || path.starts_with(&format!("{wanted}."))
                        || path.starts_with(&format!("{wanted}[")))
            } else {
                address == entity || address.starts_with(&format!("{entity}/"))
            }
        };
        let mut keys = Vec::new();
        for (key, value) in record
            .get(field::OVERRIDES)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let (inner, path) = key
                .rsplit_once('/')
                .map_or((None, key.as_str()), |(inner, path)| (Some(inner), path));
            let address = inner.map_or_else(
                || instance.to_string(),
                |inner| format!("{instance}/{inner}"),
            );
            if !selected(&address, path)
                || (inner.is_none() && matches!(path, "Transform.position" | "Transform.rotation"))
            {
                continue;
            }
            let source = super::sources::address_in_source(
                ctx.read().workspace(),
                anchor.as_str(),
                &address,
                &destination.doc,
            )?;
            apply_value(ctx, &destination.doc, &source, path, value.clone())?;
            keys.push(key.clone());
        }
        let mut removals = Vec::new();
        for removal in record
            .get(field::REMOVED)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let (inner, component) = match removal.rsplit_once('/') {
                Some((inner, component)) if is_component_key(component) => {
                    (Some(inner), Some(component))
                }
                None if is_component_key(removal) => (None, Some(removal)),
                _ => (Some(removal), None),
            };
            let address = inner.map_or_else(
                || instance.to_string(),
                |inner| format!("{instance}/{inner}"),
            );
            if !selected(&address, component.unwrap_or("")) {
                continue;
            }
            let source = super::sources::address_in_source(
                ctx.read().workspace(),
                anchor.as_str(),
                &address,
                &destination.doc,
            )?;
            apply_removal(ctx, &destination.doc, &source, component)?;
            removals.push(removal.to_owned());
        }
        if keys.is_empty() && removals.is_empty() {
            return Err(invalid("Nothing is overridden on this selection"));
        }
        for key in &keys {
            ctx.set(&instance, FieldPath::field(field::OVERRIDES).key(key), None)?;
        }
        if !removals.is_empty() {
            let kept: Vec<_> = record
                .get(field::REMOVED)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|value| {
                    !value
                        .as_str()
                        .is_some_and(|value| removals.iter().any(|removed| removed == value))
                })
                .cloned()
                .collect();
            ctx.set(
                &instance,
                FieldPath::field(field::REMOVED),
                (!kept.is_empty()).then_some(json!(kept)),
            )?;
        }
        Ok(
            json!({"source":destination.doc,"path":destination.path,"applied":keys,"removed":removals}),
        )
    }
}

fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message)
}

fn apply_value(
    ctx: &mut EditCtx<'_, '_>,
    doc: &AssetId,
    entity: &str,
    path: &str,
    value: Value,
) -> Result<(), Diagnostic> {
    ctx.in_document(doc, |ctx| {
        let parsed: FieldPath = path.parse().map_err(invalid)?;
        let target = ctx.read().target(entity)?;
        if is_component_key(parsed.head()) && !ctx.read().has_component(&target, parsed.head()) {
            let whole = path == parsed.head();
            ComponentAdd::run(
                ctx,
                ComponentAddParams {
                    entity: EntityRef(entity.into()),
                    component: parsed.head().into(),
                    value: whole.then(|| value.clone()),
                },
            )?;
            if whole {
                return Ok(());
            }
        }
        PropertySet::run(
            ctx,
            PropertySetParams {
                entity: EntityRef(entity.into()),
                path: path.into(),
                value,
            },
        )?;
        Ok(())
    })
}

fn apply_removal(
    ctx: &mut EditCtx<'_, '_>,
    doc: &AssetId,
    entity: &str,
    component: Option<&str>,
) -> Result<(), Diagnostic> {
    ctx.in_document(doc, |ctx| {
        if !ctx.read().resolved().contains_key(entity) {
            return Ok(());
        }
        if let Some(component) = component {
            let target = ctx.read().target(entity)?;
            if ctx.read().has_component(&target, component) {
                ComponentRemove::run(
                    ctx,
                    ComponentRemoveParams {
                        entity: EntityRef(entity.into()),
                        component: component.into(),
                    },
                )?;
            }
        } else {
            EntityDelete::run(
                ctx,
                EntityDeleteParams {
                    entities: vec![EntityRef(entity.into())],
                },
            )?;
        }
        Ok(())
    })
}
