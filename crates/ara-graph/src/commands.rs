//! Typed agent/editor commands. Every change uses the kernel's EditCtx and ops.

use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_doc::RecordId;
use ara_schema::{FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    Edge, Editor, Graph, Node, codes,
    model::{decode, encode},
};

/// Adds a versioned node with caller-provided or minted stable ID.
#[derive(Schema, Deserialize)]
#[ara(params, module = "graph", name = "GraphAddNodeParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddNodeParams {
    /// Optional concrete ID or batch symbol.
    pub id: Option<String>,
    /// Documented, typed node payload.
    pub node: Node,
}

/// Identifies a node to delete with all incident connections.
#[derive(Schema, Deserialize)]
#[ara(params, module = "graph", name = "GraphRemoveNodeParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveNodeParams {
    /// Stable node ID or batch symbol.
    pub node: String,
}

/// Connects two existing typed sockets.
#[derive(Schema, Deserialize)]
#[ara(params, module = "graph", name = "GraphConnectParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectParams {
    /// Optional concrete edge ID or batch symbol.
    pub id: Option<String>,
    /// Directed socket connection.
    pub edge: Edge,
}

/// Identifies an edge to disconnect.
#[derive(Schema, Deserialize)]
#[ara(params, module = "graph", name = "GraphDisconnectParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisconnectParams {
    /// Stable edge ID or batch symbol.
    pub edge: String,
}

/// Sets a literal node parameter without replacing the node.
#[derive(Schema, Deserialize)]
#[ara(params, module = "graph", name = "GraphSetParamParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetParamParams {
    /// Stable node ID or batch symbol.
    pub node: String,
    /// Parameter key.
    pub name: String,
    /// Typed by the domain adapter; preserved losslessly in generic storage.
    pub value: Value,
}

/// Moves a node in the nonsemantic editor section.
#[derive(Schema, Deserialize)]
#[ara(params, module = "graph", name = "GraphMoveNodeParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveNodeParams {
    /// Stable node ID or batch symbol.
    pub node: String,
    /// Canvas position in logical pixels.
    pub position: [f32; 2],
}

/// Replaces canvas annotations, groups and routing with an undoable op diff.
#[derive(Schema, Deserialize)]
#[ara(params, module = "graph", name = "GraphEditorParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorParams {
    /// Complete nonsemantic editor state.
    pub editor: Editor,
}

fn graph(ctx: &EditCtx<'_, '_>) -> Result<Graph, Diagnostic> {
    Graph::from_document(ctx.read().doc())
}
fn id(ctx: &EditCtx<'_, '_>, text: &str, prefix: &str) -> Result<RecordId, Diagnostic> {
    let id = ctx.read().target(text)?.record().clone();
    if id.prefix() != prefix {
        return Err(Diagnostic::error(
            codes::STRUCTURE,
            format!("expected {prefix}_ ID"),
        ));
    }
    Ok(id)
}

macro_rules! command {
    ($name:ident, $params:ty, $key:literal, $doc:literal, $example:expr, |$ctx:ident, $p:ident| $body:block) => {
        #[doc = $doc]
        pub struct $name;
        impl Command for $name {
            type Params = $params;
            type Output = Value;
            const NAME: &'static str = $key;
            const SUMMARY: &'static str = $doc;
            fn examples() -> Vec<Value> { vec![$example] }
            fn run($ctx: &mut EditCtx<'_, '_>, $p: Self::Params) -> Result<Value, Diagnostic> $body
        }
    }
}

command!(
    AddNode,
    AddNodeParams,
    "graph.node.add",
    "Add a graph node.",
    json!({"node":{"kind":"example.value","version":1,"ports":{},"params":{},"payload":null}}),
    |ctx, p| {
        let _ = graph(ctx)?;
        let id = ctx.new_id(p.id.as_deref(), "n")?;
        let value = encode(&p.node)?;
        ctx.create(id.clone(), decode(value)?)?;
        Ok(json!({"id":id}))
    }
);

command!(
    RemoveNode,
    RemoveNodeParams,
    "graph.node.remove",
    "Delete a node and its edges.",
    json!({"node":"n_a"}),
    |ctx, p| {
        let id = id(ctx, &p.node, "n")?;
        let mut next = graph(ctx)?;
        next.nodes.remove(id.as_str());
        next.edges
            .retain(|_, e| e.from.node != id.as_str() && e.to.node != id.as_str());
        next.editor.positions.remove(id.as_str());
        next.editor
            .reroutes
            .retain(|e, _| next.edges.contains_key(e));
        for group in next.editor.groups.values_mut() {
            group.nodes.retain(|n| n != id.as_str());
        }
        apply_snapshot(ctx, &next)?;
        Ok(Value::Null)
    }
);

command!(
    Connect,
    ConnectParams,
    "graph.connect",
    "Connect compatible graph sockets.",
    json!({"edge":{"from":{"node":"n_a","port":"out"},"to":{"node":"n_b","port":"in"}}}),
    |ctx, p| {
        let mut next = graph(ctx)?;
        let edge_id = ctx.new_id(p.id.as_deref(), "w")?;
        if next.edges.values().any(|e| e.to == p.edge.to) {
            return Err(Diagnostic::error(
                codes::PORT,
                "input already has a connection",
            ));
        }
        next.edges.insert(edge_id.to_string(), p.edge.clone());
        // Cycle policy belongs to the adapter; the generic command permits feedback.
        if let Some(d) = next
            .check_wiring(|a, b| a == b, true)
            .into_iter()
            .find(|d| d.location.record.as_deref() == Some(edge_id.as_str()))
        {
            return Err(d);
        }
        ctx.create(edge_id.clone(), decode(encode(&p.edge)?)?)?;
        Ok(json!({"id":edge_id}))
    }
);

command!(
    SetEditor,
    EditorParams,
    "graph.editor.set",
    "Edit graph comments, groups and reroutes.",
    json!({"editor":{}}),
    |ctx, p| {
        let mut next = graph(ctx)?;
        next.editor = p.editor;
        apply_snapshot(ctx, &next)?;
        Ok(Value::Null)
    }
);

command!(
    Disconnect,
    DisconnectParams,
    "graph.disconnect",
    "Disconnect a graph edge.",
    json!({"edge":"w_a"}),
    |ctx, p| {
        let edge = id(ctx, &p.edge, "w")?;
        ctx.delete(&edge)?;
        let layout =
            RecordId::parse("l_editor").map_err(|e| Diagnostic::error(codes::STRUCTURE, e))?;
        ctx.set(
            &layout,
            FieldPath::field("reroutes").key(edge.as_str()),
            None,
        )?;
        Ok(Value::Null)
    }
);

command!(
    SetParam,
    SetParamParams,
    "graph.param.set",
    "Set a graph node parameter.",
    json!({"node":"n_a","name":"value","value":0.5}),
    |ctx, p| {
        let id = id(ctx, &p.node, "n")?;
        if p.name.is_empty() {
            return Err(Diagnostic::error(
                codes::STRUCTURE,
                "parameter key is empty",
            ));
        }
        ctx.set(&id, FieldPath::field("params").key(p.name), Some(p.value))?;
        Ok(Value::Null)
    }
);

command!(
    MoveNode,
    MoveNodeParams,
    "graph.node.move",
    "Move a graph node without recompilation.",
    json!({"node":"n_a","position":[100,200]}),
    |ctx, p| {
        let id = id(ctx, &p.node, "n")?;
        if p.position.iter().any(|x| !x.is_finite()) {
            return Err(Diagnostic::error(
                codes::STRUCTURE,
                "position must be finite",
            ));
        }
        let layout =
            RecordId::parse("l_editor").map_err(|e| Diagnostic::error(codes::STRUCTURE, e))?;
        ctx.set(
            &layout,
            FieldPath::field("positions").key(id.as_str()),
            Some(encode(&p.position)?),
        )?;
        Ok(Value::Null)
    }
);

/// Reconciles a checked domain snapshot into field-granular kernel ops.
/// Used by text/graph projections; preserves attribution, undo and revision checks.
pub fn apply_snapshot(ctx: &mut EditCtx<'_, '_>, next: &Graph) -> Result<(), Diagnostic> {
    let current = ctx.read().doc().clone();
    if current.id != next.id || current.kind != "graph" {
        return Err(Diagnostic::error(
            codes::STRUCTURE,
            "snapshot must target the same graph",
        ));
    }
    let next = next.to_document()?;
    for id in current
        .records()
        .keys()
        .filter(|id| !next.records().contains_key(*id))
    {
        ctx.delete(id)?;
    }
    for (id, record) in next.records() {
        match current.record(id.as_str()) {
            None => ctx.create(id.clone(), record.clone())?,
            Some(old) => {
                let keys: std::collections::BTreeSet<_> = old.keys().chain(record.keys()).collect();
                for key in keys {
                    if old.get(key) != record.get(key) {
                        ctx.set(id, FieldPath::field(key), record.get(key).cloned())?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Adds the graph commands and their reflected parameter schemas to a registry.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry
        .command::<AddNode>(db)
        .command::<RemoveNode>(db)
        .command::<Connect>(db)
        .command::<Disconnect>(db)
        .command::<SetParam>(db)
        .command::<MoveNode>(db)
        .command::<SetEditor>(db);
}
