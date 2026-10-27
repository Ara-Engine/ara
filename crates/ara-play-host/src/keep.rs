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
mod planner;

impl PlayService {
    /// Enrich a read-only player diff with the same field classification as Keep.
    pub(crate) fn diff_for_keep(
        &self,
        cx: &Cx<'_>,
        world: &str,
        entities: Option<&[String]>,
        mut diff: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        let changes = changes(&diff)?;
        let scene = &self.world(world)?.scene;
        let plan = planner::classify(cx.store(), scene, changes, entities)?;
        let map = diff
            .as_object_mut()
            .ok_or_else(|| fail(codes::PLAY, "Runtime diff is not an object"))?;
        map.insert("keepCandidates".into(), json!(plan.candidates));
        map.insert("keepDiagnostics".into(), json!(plan.diagnostics));
        Ok(diff)
    }
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
        let mut changes = changes(&diff)?;
        if let Some(fields) = &p.fields {
            changes.retain(|change| {
                fields
                    .iter()
                    .any(|field| field.entity == change.entity && field.path == change.path)
            });
        }
        let plan = planner::classify(cx.store(), &scene, changes, p.entities.as_deref())?;
        let mut calls = Vec::new();
        let mut kept = Vec::new();
        let mut conflicts = Vec::new();
        let mut skipped = Vec::new();
        for change in plan.candidates {
            if !planner::selected(&p, &change) {
                continue;
            }
            if change.status == "conflict" {
                conflicts.push(json!({"entity": change.entity, "path": change.path, "edit": change.edit, "playStart": change.play_start, "play": change.play}));
                continue;
            }
            if !change.eligible {
                skipped.push(
                    json!({"entity":change.entity,"path":change.path,"reason":change.reason}),
                );
                continue;
            }
            let Some(value) = change.play else { continue };
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

fn changes(diff: &Value) -> Result<Vec<ara_runtime::live::Change>, Vec<Diagnostic>> {
    let mut changes: Vec<ara_runtime::live::Change> =
        serde_json::from_value(diff["changes"].clone())
            .map_err(|error| fail(codes::PLAY, error.to_string()))?;
    // The runtime emits field diffs with a value (including a nullable clear).
    // JSON Option<Value> collapses Some(null) to None; removals are component or
    // entity paths. Restore field nulls and let the schema decide eligibility.
    for change in &mut changes {
        if change.value.is_none() && change.path.contains('.') {
            change.value = Some(Value::Null);
        }
    }
    Ok(changes)
}

/// The stored value at `Component.field…` of `entity` (canonical, defaults omitted).
fn current_value(record: &ara_doc::Record, schema: &SchemaDb, path: &str) -> Option<Value> {
    let path = path.parse::<ara_schema::FieldPath>().ok()?;
    let resolved = schema.resolve_path(schema.get("Entity")?, &path).ok()?;
    let head = path.head();
    let ty = if ara_doc::is_component_key(head) {
        ara_schema::ValueType::Named(head.into())
    } else {
        resolved.ty.clone()
    };
    let mut root = serde_json::Map::new();
    root.insert(head.into(), schema.canonical_value(&ty, record.get(head)?));
    ara_schema::path::get(&root, &resolved.steps).cloned()
}

/// Equality at f32 precision for numbers (play values are f32 widened to JSON).
fn same(a: &Option<Value>, b: &Option<Value>) -> bool {
    fn eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                if (x.is_i64() || x.is_u64()) && (y.is_i64() || y.is_u64()) {
                    x == y
                } else {
                    x.as_f64().map(|v| v as f32) == y.as_f64().map(|v| v as f32)
                }
            }
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(a, b)| eq(a, b))
            }
            (Value::Object(x), Value::Object(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .all(|(key, value)| y.get(key).is_some_and(|other| eq(value, other)))
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
