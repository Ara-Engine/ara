//! A typed bus mutation produces ordinary invertible document ops.
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::{Diagnostic, codes};
use ara_doc::RecordId;
use ara_schema::{FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

/// Creates or replaces a complete mixer category.
#[derive(Schema, Deserialize)]
#[ara(params, module = "audio")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MixerBusSetParams {
    /// Stable bus record ID or a new batch symbol.
    pub id: String,
    /// Typed bus data, with reflected defaults for omitted fields.
    pub bus: crate::BusDesc,
}
struct SetBus;
impl Command for SetBus {
    type Params = MixerBusSetParams;
    type Output = Value;
    const NAME: &'static str = "mixer.bus.set";
    const SUMMARY: &'static str = "Create or replace a typed mixer bus (master, music, sfx).";
    fn examples() -> Vec<Value> {
        vec![json!({"id":"b_music","bus":{"name":"music","output":"master"}})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, params: Self::Params) -> Result<Value, Diagnostic> {
        let invalid = |message: String| Diagnostic::error(codes::INVALID_PARAMS, message);
        if cx.read().doc().kind != "mixer" {
            return Err(invalid("expected a mixer document".into()));
        }
        let record = ara_json::to_value(&params.bus)
            .map_err(|e| invalid(e.to_string()))?
            .as_object()
            .cloned()
            .ok_or_else(|| invalid("expected bus record".into()))?;
        let id = if cx.read().doc().record(&params.id).is_some() {
            let id = RecordId::parse(&params.id).map_err(invalid)?;
            if id.prefix() != "b" {
                return Err(invalid("mixer bus IDs use the b prefix".into()));
            }
            for (key, value) in record {
                cx.set(&id, FieldPath::field(key), Some(value))?;
            }
            id
        } else {
            let id = cx.new_id(Some(&params.id), "b")?;
            cx.create(id.clone(), record)?;
            id
        };
        Ok(json!({"id":id}))
    }
}

/// Registers the CLI/MCP/editor mixer authoring command and its parameter schema.
pub fn register(registry: &mut Registry, schema: &mut SchemaDb) {
    registry.command::<SetBus>(schema);
}
