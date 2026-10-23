//! All live effect edits produce attributed, invertible kernel ops.
use crate::{Effect, Stage, decode, error};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_graph::Graph;
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

/// Complete ID-free source edit, suitable for agents or a text editor.
#[derive(Schema, Deserialize)]
#[ara(params, name = "VfxTextParams", module = "vfx")]
#[serde(deny_unknown_fields)]
pub struct TextParams {
    /// Canonical ID-free effect JSON.
    pub source: String,
}
/// A structured stack edit; the operation and module are domain-validated.
#[derive(Schema, Deserialize)]
#[ara(params, name = "VfxStackParams", module = "vfx")]
#[serde(deny_unknown_fields)]
pub struct StackParams {
    /// Stable emitter name.
    pub emitter: String,
    /// One of spawn, initialize, update, render.
    pub stage: String,
    /// Insert, remove or move.
    pub operation: String,
    /// Stable existing module name for remove/move.
    pub name: String,
    /// Destination position for insert/move.
    pub index: Option<u32>,
    /// Typed StackModule JSON for insertion.
    pub module: Option<Value>,
}
/// Typed default parameter mutation.
#[derive(Schema, Deserialize)]
#[ara(params, name = "VfxParameterParams", module = "vfx")]
#[serde(deny_unknown_fields)]
pub struct ParameterParams {
    /// Full namespaced parameter address.
    pub key: String,
    /// Typed value object, e.g. {type:float,value:3}.
    pub value: Value,
}
fn current(ctx: &EditCtx<'_, '_>) -> Result<(Graph, Effect), Diagnostic> {
    let g = Graph::from_document(ctx.read().doc())?;
    let e = Effect::from_graph(&g)?;
    Ok((g, e))
}
fn apply(ctx: &mut EditCtx<'_, '_>, graph: Graph, effect: Effect) -> Result<Value, Diagnostic> {
    effect.resolve(&Default::default())?;
    let next = effect.to_graph(graph.id, graph.editor)?;
    ara_graph::commands::apply_snapshot(ctx, &next)?;
    Ok(json!({"hash":effect.content_hash()?,"diagnostics":effect.diagnostics()}))
}
/// Reconcile text into existing named graph nodes; changes remain undoable.
pub struct ApplyText;
impl Command for ApplyText {
    type Params = TextParams;
    type Output = Value;
    const NAME: &'static str = "vfx.text.apply";
    const SUMMARY: &'static str = "Apply typed effect text as reversible graph ops.";
    fn examples() -> Vec<Value> {
        vec![json!({"source":crate::samples::campfire().text().expect("sample serializes")})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: TextParams) -> Result<Value, Diagnostic> {
        let graph = Graph::from_document(ctx.read().doc())?;
        apply(ctx, graph, Effect::parse(&p.source)?)
    }
}
/// Insert, remove or reorder modules with stable name identities.
pub struct EditStack;
impl Command for EditStack {
    type Params = StackParams;
    type Output = Value;
    const NAME: &'static str = "vfx.stack.edit";
    const SUMMARY: &'static str = "Insert, remove or reorder a named VFX module.";
    fn examples() -> Vec<Value> {
        vec![json!({"emitter":"flame","stage":"update","operation":"move","name":"drag","index":0})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: StackParams) -> Result<Value, Diagnostic> {
        let (graph, mut effect) = current(ctx)?;
        let stage: Stage = decode(Value::String(p.stage))?;
        let e = effect
            .emitters
            .iter_mut()
            .find(|e| e.name == p.emitter)
            .ok_or_else(|| error("unknown emitter"))?;
        let stack = e.stacks.get_mut(stage);
        match p.operation.as_str() {
            "remove" => {
                let index = stack
                    .iter()
                    .position(|m| m.name == p.name)
                    .ok_or_else(|| error("unknown module"))?;
                stack.remove(index);
            }
            "insert" | "move" => {
                let index = p.index.ok_or_else(|| error("index required"))? as usize;
                let module = if p.operation == "move" {
                    let old = stack
                        .iter()
                        .position(|m| m.name == p.name)
                        .ok_or_else(|| error("unknown module"))?;
                    stack.remove(old)
                } else {
                    decode(p.module.ok_or_else(|| error("module required"))?)?
                };
                if index > stack.len() {
                    return Err(error("stack index out of range"));
                }
                stack.insert(index, module);
            }
            _ => return Err(error("expected insert, remove or move")),
        }
        apply(ctx, graph, effect)
    }
}
/// Change a declared parameter default without altering its type.
pub struct SetParameter;
impl Command for SetParameter {
    type Params = ParameterParams;
    type Output = Value;
    const NAME: &'static str = "vfx.parameter.set";
    const SUMMARY: &'static str = "Set a typed VFX parameter default.";
    fn examples() -> Vec<Value> {
        vec![json!({"key":"user.intensity","value":{"type":"float","value":1}})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: ParameterParams) -> Result<Value, Diagnostic> {
        let (graph, mut effect) = current(ctx)?;
        let next: crate::Value = decode(p.value)?;
        let parameter = effect
            .parameters
            .iter_mut()
            .find(|v| v.key() == p.key)
            .ok_or_else(|| error("unknown parameter"))?;
        if std::mem::discriminant(&next) != std::mem::discriminant(&parameter.default) {
            return Err(error("parameter type cannot change through a value edit"));
        }
        parameter.default = next;
        apply(ctx, graph, effect)
    }
}
/// Register effect commands in the same registry used by the editor and MCP.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry
        .command::<ApplyText>(db)
        .command::<EditStack>(db)
        .command::<SetParameter>(db);
}
