//! "Keep changes": play diff → filter → `prev` conflict check → one transaction.
use crate::{PlayService, codes, fail, params::PlayKeepParams};
use ara_diag::Diagnostic;
use ara_host::{
    Cx,
    params::{ApplyParams, CallSpec},
};
use ara_play::method;
use ara_schema::SchemaDb;
use serde_json::{Value, json};

impl PlayService {
    /// "Keep changes": diff → filter → `prev` conflict check → one transaction.
    pub(crate) fn keep(
        &mut self,
        cx: &mut Cx<'_>,
        p: PlayKeepParams,
    ) -> Result<Value, Vec<Diagnostic>> {
        let world = match p.world.clone() {
            Some(world) => world,
            None => self
                .latest()
                .ok_or_else(|| fail(codes::UNKNOWN_WORLD, "no play world is running"))?,
        };
        let diff = self.call(&world, method::DIFF, json!({}), false)?;
        self.keep_from_diff(cx, p, diff)
    }

    /// The asynchronous path rejoins its live initiating session here. Diff filtering,
    /// conflict checks and the one typed Cx.apply transaction are shared by both paths.
    pub(crate) fn keep_from_diff(
        &mut self,
        cx: &mut Cx<'_>,
        p: PlayKeepParams,
        diff: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        let world = p
            .world
            .clone()
            .or_else(|| self.latest())
            .ok_or_else(|| fail(codes::UNKNOWN_WORLD, "No Play world is running"))?;
        let scene = self.world(&world)?.scene.clone();
        let changes: Vec<ara_runtime::live::Change> =
            serde_json::from_value(diff["changes"].clone())
                .map_err(|e| fail(codes::PLAY, e.to_string()))?;
        let include_driven = p.include_driven.unwrap_or(false);
        let doc = cx
            .store()
            .workspace()
            .docs()
            .find(|(_, d)| d.id.as_str() == scene)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| {
                fail(
                    codes::REQUEST,
                    format!("scene `{scene}` is no longer loaded"),
                )
            })?;
        let schema = cx.store().schema().clone();
        let mut calls = Vec::new();
        let mut kept = Vec::new();
        let mut conflicts = Vec::new();
        let mut skipped = Vec::new();
        for change in changes {
            let wanted = (include_driven || !change.driven)
                && p.entities
                    .as_ref()
                    .is_none_or(|e| e.contains(&change.entity))
                && p.paths
                    .as_ref()
                    .is_none_or(|paths| paths.iter().any(|q| change.path.starts_with(q.as_str())));
            if !wanted {
                continue;
            }
            let Some(value) = change.value.clone() else {
                skipped.push(json!({"entity": change.entity, "path": change.path, "reason": "removed in play"}));
                continue;
            };
            if change.path.is_empty() {
                skipped.push(json!({"entity": change.entity, "reason": "deleted in play"}));
                continue;
            }
            // The edit world may have changed since Play: `prev` must still hold.
            let current = current_value(&doc, &schema, &change.entity, &change.path);
            if !same(&current, &change.prev) {
                conflicts.push(json!({"entity": change.entity, "path": change.path, "edit": current, "playStart": change.prev, "play": value}));
                continue;
            }
            calls.push(CallSpec {
                cmd: "property.set".into(),
                args: Some(json!({"entity": change.entity, "path": change.path, "value": value})),
                symbol: None,
            });
            kept.push(json!({"entity": change.entity, "path": change.path, "value": value}));
        }
        if calls.is_empty() {
            return Ok(
                json!({"kept": kept, "conflicts": conflicts, "skipped": skipped, "transaction": null}),
            );
        }
        let session = cx
            .session()
            .map(|s| s.id.clone())
            .ok_or_else(|| fail(codes::REQUEST, "keep needs a session"))?;
        let result = cx.apply(ApplyParams {
            commands: calls,
            doc: Some(scene),
            session,
            why: Some(
                p.why
                    .unwrap_or_else(|| format!("Keep changes from {world}")),
            ),
            dry_run: None,
            base: None,
            idempotency_key: None,
            branch: None,
        })?;
        Ok(json!({"kept": kept, "conflicts": conflicts, "skipped": skipped, "transaction": result}))
    }
}

/// The stored value at `Component.field…` of `entity` (canonical, defaults omitted).
fn current_value(
    doc: &ara_doc::Document,
    schema: &SchemaDb,
    entity: &str,
    path: &str,
) -> Option<Value> {
    let record = doc.record(entity)?;
    let mut parts = path.split('.');
    let component = parts.next()?;
    let ty = ara_schema::ValueType::Named(component.into());
    let mut value = schema.canonical_value(&ty, record.get(component)?);
    for part in parts {
        value = value.get(part)?.clone();
    }
    Some(value)
}

/// Equality at f32 precision for numbers (play values are f32 widened to JSON).
fn same(a: &Option<Value>, b: &Option<Value>) -> bool {
    fn eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                x.as_f64().map(|v| v as f32) == y.as_f64().map(|v| v as f32)
            }
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(a, b)| eq(a, b))
            }
            _ => a == b,
        }
    }
    match (a, b) {
        (Some(a), Some(b)) => eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
